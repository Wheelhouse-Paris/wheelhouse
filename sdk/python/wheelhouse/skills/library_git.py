"""LibraryGit — per-file git history SDK methods for Library explorer.

Exposes git operations (tree browsing, file content retrieval,
per-file commit history, content search, and file restore) on a
:class:`LibrarySandbox` instance. Every path argument is validated
through the sandbox's ``_validate`` method before being passed to git,
so no path traversal is possible (ADR-050, constraint LE-03).

All methods shell out to ``git`` via :mod:`subprocess` — no ``pygit2``
dependency (ADR-050 rejected it to avoid native dependencies in the
Lambda layer).

See:
    - epics-library-explorer.md#Story 15.1.2, 15.1.3, 15.1.4
    - architecture.md#ADR-050 (Per-File Git History SDK Methods)
    - architecture.md#ADR-040 (Concurrent Write Serialization)

Public API:
    LibraryGit(sandbox)
        .tree(sha=None) -> list[TreeEntry]
        .file_content(path, sha=None) -> bytes
        .file_log(path, limit=None) -> list[LogEntry]
        .search(query) -> list[SearchResult]
        .restore(path, sha) -> str
"""

from __future__ import annotations

import dataclasses
import os
import re
import subprocess
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from wheelhouse.skills.library_sandbox import LibrarySandbox

from wheelhouse.errors import LibraryCommitError
from wheelhouse.skills.library_sandbox import _sanitize_git_stderr

# SHA validation: 4-40 hex characters (case-insensitive).
# Prevents command injection via malicious SHA strings.
_SHA_RE = re.compile(r"^[0-9a-fA-F]{4,40}$")

# Git subprocess timeout — matches library_sandbox._GIT_SUBPROCESS_TIMEOUT_S.
_GIT_SUBPROCESS_TIMEOUT_S = 30.0


def _validate_sha(sha: str) -> None:
    """Raise ValueError if sha is not a valid hex git reference."""
    if not _SHA_RE.match(sha):
        raise ValueError(
            f"Invalid SHA format: expected 4-40 hex characters, got {sha!r}"
        )


@dataclasses.dataclass(frozen=True)
class TreeEntry:
    """A single entry in a git tree listing.

    Attributes:
        path: Relative path within the Library (forward-slash separated).
        type: ``"file"`` or ``"dir"``.
        size: File size in bytes (0 for directories).
        last_modified: Unix timestamp (seconds) of the last commit
            that touched this file, or 0 if unknown.
    """

    path: str
    type: str  # "file" or "dir"
    size: int
    last_modified: int


@dataclasses.dataclass(frozen=True)
class LogEntry:
    """A single commit entry from a file's history.

    Attributes:
        sha: Full 40-character commit SHA.
        author: Author name (typically ``Agent <name>``).
        timestamp: Unix timestamp (seconds) of the commit.
        message: First line of the commit message.
        diff_stat: Lines added/removed summary, e.g. ``"+12 -3"``.
            Empty string if stats are unavailable.
    """

    sha: str
    author: str
    timestamp: int
    message: str
    diff_stat: str


@dataclasses.dataclass(frozen=True)
class SearchResult:
    """A single match from a content search.

    Attributes:
        path: Relative path of the matching file.
        line_number: 1-based line number of the match.
        matching_line: The full text of the matching line (stripped).
        context_before: Up to 2 lines before the match.
        context_after: Up to 2 lines after the match.
    """

    path: str
    line_number: int
    matching_line: str
    context_before: list[str]
    context_after: list[str]


class LibraryGit:
    """Read-only git operations on a Library sandbox.

    Args:
        sandbox: A :class:`LibrarySandbox` instance. The sandbox's
            ``_validate`` method is used for path containment checks,
            and ``_root`` provides the working directory for git commands.
    """

    def __init__(self, sandbox: LibrarySandbox) -> None:
        self._sandbox = sandbox

    # ─── Public API ────────────────────────────────────────────────────

    def tree(self, *, sha: str | None = None) -> list[TreeEntry]:
        """Return the directory tree at HEAD (or at a historical commit).

        Each entry includes path, type, size, and last_modified timestamp.
        Files and directories are listed; the listing is fully recursive.

        Args:
            sha: Optional commit SHA to inspect. Defaults to HEAD.

        Returns:
            Sorted list of :class:`TreeEntry` objects.

        Raises:
            ValueError: if ``sha`` is not a valid hex reference.
            LibraryCommitError: if the git command fails.
        """
        ref = "HEAD"
        if sha is not None:
            _validate_sha(sha)
            ref = sha

        # Get file listing with sizes.
        # Format: <mode> SP <type> SP <object> SP <size> TAB <path>
        result = self._git_text(["ls-tree", "-r", "--long", ref])
        if not result.strip():
            return []

        entries: list[TreeEntry] = []
        paths: list[str] = []

        for line in result.strip().splitlines():
            # Parse: "100644 blob <sha>    <size>\t<path>"
            # The size field is right-justified and padded with spaces.
            meta, path = line.split("\t", 1)
            parts = meta.split()
            # parts: [mode, type, object_sha, size]
            git_type = parts[1]
            size_str = parts[3]
            entry_type = "file" if git_type == "blob" else "dir"
            size = int(size_str) if size_str != "-" else 0

            entries.append(
                TreeEntry(
                    path=path,
                    type=entry_type,
                    size=size,
                    last_modified=0,  # filled below
                )
            )
            paths.append(path)

        # Batch-fetch last_modified timestamps. For Library-scale repos
        # (typically <500 files) this is acceptable. Each file gets one
        # git log call for its last commit timestamp.
        timestamps = self._batch_last_modified(paths, ref)

        # Replace entries with correct timestamps.
        result_entries = []
        for entry in entries:
            ts = timestamps.get(entry.path, 0)
            result_entries.append(
                dataclasses.replace(entry, last_modified=ts)
            )

        result_entries.sort(key=lambda e: e.path)
        return result_entries

    def file_content(self, path: str, *, sha: str | None = None) -> bytes:
        """Return raw file content at HEAD (or at a historical commit).

        Binary-safe: returns raw ``bytes``, not ``str`` (ADR-050 LE-04).
        The caller determines encoding from the file extension.

        Args:
            path: Relative path within the Library.
            sha: Optional commit SHA. Defaults to HEAD.

        Returns:
            Raw file content as bytes.

        Raises:
            PathEscapeError: if ``path`` resolves outside the sandbox.
            FileNotFoundError: if the file does not exist at the given ref.
            ValueError: if ``sha`` is not a valid hex reference.
            LibraryCommitError: if the git command fails for other reasons.
        """
        # Validate path through sandbox (raises PathEscapeError on traversal).
        self._sandbox._validate(path)

        ref = "HEAD"
        if sha is not None:
            _validate_sha(sha)
            ref = sha

        git_path = f"{ref}:{path}"
        try:
            return self._git_bytes(["show", git_path])
        except LibraryCommitError as exc:
            msg = str(exc)
            if "does not exist" in msg or "not exist in" in msg or "exists on disk" in msg:
                raise FileNotFoundError(
                    f"File '{path}' does not exist at revision '{ref}'"
                ) from exc
            raise

    def file_log(
        self, path: str, *, limit: int | None = None
    ) -> list[LogEntry]:
        """Return the commit history for a single file.

        Each entry includes the commit SHA, author, timestamp, message,
        and a diff stat summary (lines added/removed).

        Args:
            path: Relative path within the Library.
            limit: Maximum number of entries to return.  ``None`` means
                all commits.

        Returns:
            List of :class:`LogEntry` objects, most recent first.

        Raises:
            PathEscapeError: if ``path`` resolves outside the sandbox.
            LibraryCommitError: if the git command fails.
        """
        # Validate path through sandbox (raises PathEscapeError on traversal).
        self._sandbox._validate(path)

        args = ["log", "--format=%H|%an|%at|%s", "--numstat", "--"]
        if limit is not None:
            args = [
                "log",
                f"-n{limit}",
                "--format=%H|%an|%at|%s",
                "--numstat",
                "--",
            ]
        args.append(path)

        output = self._git_text(args)
        return self._parse_log_with_numstat(output)

    def search(self, query: str) -> list[SearchResult]:
        """Search all tracked Library content for a literal string.

        Uses ``git grep`` with ``--fixed-strings`` so that the query is
        treated as a literal string (no regex interpretation, preventing
        injection).  Binary files are excluded via ``-I``.

        Args:
            query: The literal string to search for.

        Returns:
            List of :class:`SearchResult` objects.  Returns an empty list
            when there are no matches (not an error).

        Raises:
            LibraryCommitError: if the git command fails for reasons
                other than "no match".
        """
        if not query:
            return []

        env = self._git_env()
        result = subprocess.run(  # noqa: S603
            [
                "git",
                "grep",
                "-n",       # line numbers
                "-I",       # skip binary files
                "-C2",      # 2 lines of context
                "--fixed-strings",
                "--",
                query,
            ],
            cwd=self._sandbox._root,
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=_GIT_SUBPROCESS_TIMEOUT_S,
        )

        # Exit code 1 = no matches (normal).
        if result.returncode == 1:
            return []

        if result.returncode != 0:
            stderr = _sanitize_git_stderr(result.stderr or "")
            raise LibraryCommitError(
                f"git grep failed: {stderr.strip() or '<no stderr>'}"
            )

        return self._parse_grep_output(result.stdout)

    def restore(self, path: str, *, sha: str) -> str:
        """Restore a file to its content at a historical commit.

        Checks out the file at the given SHA and creates a new commit
        recording the restore operation. The commit message follows
        ADR-039 structured format with a ``restored_from`` trailer.

        Uses ``git checkout <sha> -- <path>`` which writes to the
        working tree and stages the file in one operation, then commits
        via the sandbox's lock-retry wrapper (ADR-040).

        Args:
            path: Relative path within the Library.
            sha: Commit SHA to restore from.

        Returns:
            The full 40-character SHA of the newly created commit.

        Raises:
            PathEscapeError: if ``path`` resolves outside the sandbox.
            ValueError: if ``sha`` is not a valid hex reference.
            FileNotFoundError: if the file does not exist at the given
                SHA (or the SHA itself does not exist).
            LibraryBusyError: if the git index lock cannot be acquired
                within the timeout (ADR-040).
            LibraryCommitError: if any git command fails for other reasons.
        """
        # 1. Validate path through sandbox (raises PathEscapeError on traversal).
        self._sandbox._validate(path)

        # 2. Validate SHA format.
        _validate_sha(sha)

        # 3. Verify the file exists at the given SHA.
        self._check_file_exists_at_sha(path, sha)

        # 4. Checkout the file at the historical SHA.
        # `git checkout <sha> -- <path>` writes to the working tree AND
        # stages the file in the index in one operation.
        self._sandbox._git_with_lock_retry(["checkout", sha, "--", path])

        # 5. Build structured commit message (ADR-039).
        short_sha = sha[:7]
        subject = f"[restore] Restored {path} from {short_sha}"
        message = f"{subject}\n\nrestored_from: {sha}"

        # 6. Commit with lock-retry (ADR-040 serialization).
        try:
            self._sandbox._git_with_lock_retry(
                ["commit", "-m", message, "--allow-empty"]
            )
        except BaseException:
            # Best-effort rollback: restore working tree to HEAD state.
            try:
                self._sandbox._git(
                    ["reset", "--hard", "HEAD"],
                    suppress_stderr=True,
                )
            except Exception:  # noqa: BLE001
                pass
            raise

        # 7. Return the new commit SHA.
        result = self._git_text(["rev-parse", "HEAD"])
        return result.strip()

    def _check_file_exists_at_sha(self, path: str, sha: str) -> None:
        """Raise FileNotFoundError if ``path`` does not exist at ``sha``.

        Uses ``git cat-file -e <sha>:<path>`` which exits 0 if the object
        exists and non-zero otherwise. This covers both the case where
        the SHA is valid but the file was not present, and the case where
        the SHA itself does not exist in the repository.
        """
        env = self._git_env()
        result = subprocess.run(  # noqa: S603
            ["git", "cat-file", "-e", f"{sha}:{path}"],
            cwd=self._sandbox._root,
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=_GIT_SUBPROCESS_TIMEOUT_S,
        )
        if result.returncode != 0:
            raise FileNotFoundError(
                f"File '{path}' does not exist at revision '{sha}'"
            )

    # ─── Internal helpers ──────────────────────────────────────────────

    def _git_text(self, args: list[str]) -> str:
        """Run a git command and return decoded stdout."""
        env = self._git_env()
        result = subprocess.run(  # noqa: S603
            ["git", *args],
            cwd=self._sandbox._root,
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=_GIT_SUBPROCESS_TIMEOUT_S,
        )
        if result.returncode != 0:
            stderr = _sanitize_git_stderr(result.stderr or "")
            raise LibraryCommitError(
                f"git {args[0] if args else ''} failed: {stderr.strip() or '<no stderr>'}"
            )
        return result.stdout

    def _git_bytes(self, args: list[str]) -> bytes:
        """Run a git command and return raw stdout bytes (binary-safe)."""
        env = self._git_env()
        result = subprocess.run(  # noqa: S603
            ["git", *args],
            cwd=self._sandbox._root,
            env=env,
            capture_output=True,
            text=False,
            check=False,
            timeout=_GIT_SUBPROCESS_TIMEOUT_S,
        )
        if result.returncode != 0:
            raw_stderr = (result.stderr or b"").decode("utf-8", errors="replace")
            stderr = _sanitize_git_stderr(raw_stderr)
            raise LibraryCommitError(
                f"git {args[0] if args else ''} failed: {stderr.strip() or '<no stderr>'}"
            )
        return result.stdout

    def _git_env(self) -> dict[str, str]:
        """Return environment dict for git subprocess calls."""
        env = os.environ.copy()
        env["GIT_AUTHOR_NAME"] = f"Agent {self._sandbox._agent_name}"
        env["GIT_AUTHOR_EMAIL"] = f"{self._sandbox._agent_name}@wheelhouse.dev"
        env["GIT_COMMITTER_NAME"] = f"Agent {self._sandbox._agent_name}"
        env["GIT_COMMITTER_EMAIL"] = f"{self._sandbox._agent_name}@wheelhouse.dev"
        return env

    def _batch_last_modified(
        self, paths: list[str], ref: str
    ) -> dict[str, int]:
        """Return {path: unix_timestamp} for the last commit touching each path.

        Uses one ``git log`` call per path. For Library-scale repos
        (<500 files) this is acceptable performance.
        """
        timestamps: dict[str, int] = {}
        for path in paths:
            try:
                out = self._git_text(
                    ["log", "-1", "--format=%at", ref, "--", path]
                )
                ts_str = out.strip()
                if ts_str:
                    timestamps[path] = int(ts_str)
            except (LibraryCommitError, ValueError):
                # If we can't get the timestamp, leave as 0.
                pass
        return timestamps

    @staticmethod
    def _parse_log_with_numstat(output: str) -> list[LogEntry]:
        """Parse ``git log --format='%H|%an|%at|%s' --numstat`` output.

        The format produces blocks like::

            <sha>|<author>|<timestamp>|<message>
            (blank line)
            <added>\\t<removed>\\t<path>
            (blank line)
            <sha>|<author>|<timestamp>|<message>
            ...

        A header line is identified by containing ``|`` and starting with
        a 40-char hex SHA. Numstat lines use tabs as separators.

        Returns a list of :class:`LogEntry`, most recent first.
        """
        if not output.strip():
            return []

        entries: list[LogEntry] = []
        current_header: str | None = None
        numstat_added = 0
        numstat_removed = 0

        def _is_header(line: str) -> bool:
            """Check if a line looks like a commit header (sha|author|ts|msg)."""
            if "|" not in line:
                return False
            sha_part = line.split("|", 1)[0]
            return bool(_SHA_RE.match(sha_part))

        def _is_numstat(line: str) -> bool:
            """Check if a line is a numstat entry (added\\tremoved\\tpath)."""
            parts = line.split("\t", 2)
            if len(parts) != 3:
                return False
            # Added/removed are digits or "-" (for binary).
            return (parts[0].isdigit() or parts[0] == "-") and (
                parts[1].isdigit() or parts[1] == "-"
            )

        for line in output.splitlines():
            if not line:
                continue

            if _is_numstat(line):
                parts = line.split("\t", 2)
                try:
                    numstat_added += int(parts[0])
                    numstat_removed += int(parts[1])
                except ValueError:
                    # Binary file shows "-\t-\t<path>", skip.
                    pass
                continue

            if _is_header(line):
                # Flush previous commit if any.
                if current_header is not None:
                    entries.append(
                        _build_log_entry(
                            current_header, numstat_added, numstat_removed
                        )
                    )
                    numstat_added = 0
                    numstat_removed = 0
                current_header = line
                continue

            # Unknown line — ignore.

        # Flush last commit.
        if current_header is not None:
            entries.append(
                _build_log_entry(current_header, numstat_added, numstat_removed)
            )

        return entries

    @staticmethod
    def _parse_grep_output(output: str) -> list[SearchResult]:
        """Parse ``git grep -n -I -C2 --fixed-strings`` output.

        Output groups are separated by ``--`` lines.  Within a group:

        - Match lines:   ``<path>:<linenum>:<content>``
        - Context lines: ``<path>-<linenum>-<content>``
        """
        if not output.strip():
            return []

        results: list[SearchResult] = []
        # Split into groups by the "--" separator.
        groups = re.split(r"^--$", output.strip(), flags=re.MULTILINE)

        for group in groups:
            group = group.strip()
            if not group:
                continue

            lines_before: list[str] = []
            lines_after: list[str] = []
            match_path: str | None = None
            match_lineno: int | None = None
            match_text: str | None = None
            found_match = False

            for raw_line in group.splitlines():
                # Try match line first (colon separator): path:num:content
                m = re.match(r"^(.+?):(\d+):(.*)$", raw_line)
                if m and not found_match:
                    match_path = m.group(1)
                    match_lineno = int(m.group(2))
                    match_text = m.group(3)
                    found_match = True
                    continue

                # Context line (hyphen separator): path-num-content
                ctx = re.match(r"^(.+?)-(\d+)-(.*)$", raw_line)
                if ctx:
                    content = ctx.group(3)
                    if not found_match:
                        lines_before.append(content)
                    else:
                        lines_after.append(content)
                    continue

                # Additional match lines in the same group (after first match).
                if m and found_match:
                    # Flush current match, start accumulating a new one.
                    if match_path is not None and match_lineno is not None:
                        results.append(
                            SearchResult(
                                path=match_path,
                                line_number=match_lineno,
                                matching_line=match_text or "",
                                context_before=list(lines_before),
                                context_after=list(lines_after),
                            )
                        )
                    lines_before = list(lines_after)
                    lines_after = []
                    match_path = m.group(1)
                    match_lineno = int(m.group(2))
                    match_text = m.group(3)

            if match_path is not None and match_lineno is not None:
                results.append(
                    SearchResult(
                        path=match_path,
                        line_number=match_lineno,
                        matching_line=match_text or "",
                        context_before=lines_before,
                        context_after=lines_after,
                    )
                )

        return results


def _build_log_entry(header: str, added: int, removed: int) -> LogEntry:
    """Build a LogEntry from a parsed header line and accumulated numstat."""
    parts = header.split("|", 3)
    if len(parts) < 4:
        # Malformed — best-effort.
        return LogEntry(
            sha=parts[0] if parts else "",
            author=parts[1] if len(parts) > 1 else "",
            timestamp=int(parts[2]) if len(parts) > 2 else 0,
            message="",
            diff_stat="",
        )
    sha, author, ts_str, message = parts
    diff_stat = ""
    if added or removed:
        diff_stat = f"+{added} -{removed}"
    return LogEntry(
        sha=sha,
        author=author,
        timestamp=int(ts_str) if ts_str else 0,
        message=message,
        diff_stat=diff_stat,
    )
