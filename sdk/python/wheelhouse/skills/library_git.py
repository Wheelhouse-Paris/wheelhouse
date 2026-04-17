"""LibraryGit — per-file git history SDK methods for Library explorer.

Exposes read-only git operations (tree browsing, file content retrieval)
on a :class:`LibrarySandbox` instance. Every path argument is validated
through the sandbox's ``_validate`` method before being passed to git,
so no path traversal is possible (ADR-050, constraint LE-03).

All methods shell out to ``git`` via :mod:`subprocess` — no ``pygit2``
dependency (ADR-050 rejected it to avoid native dependencies in the
Lambda layer).

See:
    - epics-library-explorer.md#Story 15.1.2
    - architecture.md#ADR-050 (Per-File Git History SDK Methods)

Public API:
    LibraryGit(sandbox)
        .tree(sha=None) -> list[TreeEntry]
        .file_content(path, sha=None) -> bytes

Story 15.1.3 will add: file_log(), search()
Story 15.1.4 will add: restore()
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
