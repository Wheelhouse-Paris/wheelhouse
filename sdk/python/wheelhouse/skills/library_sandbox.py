"""LibrarySandbox — structural filesystem boundary for Library skills.

Every Library filesystem operation (read, write, list, delete, exists)
is routed through a :class:`LibrarySandbox` instance bound to a single
Library root. Path canonicalization via :func:`os.path.realpath` plus a
:func:`os.path.commonpath` containment check ensures that skill code
cannot escape the root — neither via ``..`` traversal, nor via symlinks
inside the root, nor via absolute-path injection, nor via
sibling-prefix attacks (e.g. ``/tmp/libA`` vs ``/tmp/libA-evil``).

See:
    - epics-library.md#Story FW-1.1
    - architecture.md#ADR-036 (Library Workspace Volume and Mount Strategy)
    - architecture.md#ADR-039 (Library Git Commit Strategy)
    - architecture.md#ADR-040 (Library Concurrent Write Serialization)
    - prd.md FR14, FR22, FR38, NFR5, NFR7, NFR9, NFR17, NFR18, NFR19, NFR22

Public API:
    LibrarySandbox(library_root)
        .read(rel_path) -> str
        .write(rel_path, content) -> None
        .list(rel_path=".") -> list[str]
        .delete(rel_path) -> None
        .exists(rel_path) -> bool

All operations raise :class:`wheelhouse.errors.PathEscapeError` when the
requested path resolves outside the Library root. The error string is
sanitized and does not contain the attempted raw path (NFR9).
"""

from __future__ import annotations

import contextlib
import dataclasses
import hashlib
import logging
import os
import re
import shutil
import subprocess
import time
from typing import Any, Callable, Iterator, Union

from wheelhouse.errors import (
    LibraryBusyError,
    LibraryCommitError,
    LibraryDiskFullError,
    LibraryTransactionError,
    PathEscapeError,
)

# Story 13-6: module-scoped named logger. Library code is a guest in the
# host application's logging configuration — we never call basicConfig.
logger = logging.getLogger("wheelhouse.library_sandbox")

PathLike = Union[str, "os.PathLike[str]"]

# Pre-commit disk-space headroom (NFR22). Conservative single-commit slack
# for a 500-page repo (~2 MB worst case) with 5× safety margin. Documented
# in story 13-4 Dev Notes; parameterizable in 13-10 if size-limit
# enforcement needs a different value.
_DEFAULT_MIN_FREE_BYTES = 10 * 1024 * 1024  # 10 MB

# Subprocess timeout for any single git invocation. The 5-minute stale
# lock window (story 13-6, ADR-040) is enforced separately by the retry
# wrapper inspecting `index.lock` mtime — an individual git call still
# fails fast at 30s if it hangs for any other reason.
_GIT_SUBPROCESS_TIMEOUT_S = 30.0

# Stale-lock timeout for `.library/.git/index.lock` per ADR-040 (Story 13-5).
# A healthy 50K-word ingest takes 60–180s; 5 minutes gives 2–5× headroom
# before declaring the lock stale. Single source of truth — Story 13-6
# reuses this value (mirrored as `_LOCK_STALE_AFTER_S` below for its
# retry loop) rather than re-declaring a separate timeout.
_DEFAULT_STALE_LOCK_TIMEOUT_S = 300.0

# Module logger used by the boot-time crash-recovery sweep (Story 13-5) to
# emit operator-visible warnings when the working tree is reset or a stale
# `index.lock` is removed. Configured via the standard `logging` module by
# the agent runtime — no handlers attached here.
_LOGGER = logging.getLogger("wheelhouse.library_sandbox")

# Story 13-6 — Concurrent write serialization (FR22, NFR18, ADR-040).
#
# `_INDEX_LOCK_REL` is the path of git's native index lock relative to
# the Library root. When two writers race, the loser of the race sees a
# `LibraryCommitError` whose stderr matches `_LOCK_COLLISION_RE` — that
# is the signal we use to enter the retry loop. Two classes of race are
# retried: index.lock create failures AND the `cannot lock ref 'HEAD'`
# CAS failure that surfaces when `git commit` finishes its commit-object
# write but loses the HEAD-advance race to another thread. See the
# `_LOCK_COLLISION_RE` doc block below. Read-only git calls
# (`rev-parse --verify HEAD`) do NOT touch either lock and are routed
# through the bare `_git()` path on purpose, see `_has_head`.
_INDEX_LOCK_REL = ".git/index.lock"
_LOCK_STALE_AFTER_S = _DEFAULT_STALE_LOCK_TIMEOUT_S  # 5-minute stale window
_LOCK_RETRY_WAIT_S = 2.0  # ADR-040: 2-second backoff between attempts
_LOCK_MAX_ATTEMPTS = 3  # ADR-040: 3 fresh-wait attempts before busy

# Git's stderr signatures for the two concurrent-write race classes
# that story 13-6 retries:
#
#   1. `Unable to create <path>/index.lock`
#      The classic `git add` / `git commit` index-lock collision: another
#      process (or thread) is mid-write to the index, we arrived between
#      their `.git/index.lock` create and rename.
#
#   2. `cannot lock ref 'HEAD'`
#      The ref-update CAS race: `git commit` writes the commit object
#      successfully, then tries to atomically advance HEAD from the sha it
#      read at commit-prep time to the new commit. If another thread
#      advanced HEAD in the meantime, the CAS fails with this exact
#      message and git aborts WITHOUT moving HEAD. Retrying the full
#      `git commit` is safe and correct: the next attempt re-reads HEAD,
#      rebuilds the tree from the (now consistent) index on top of the
#      new parent, and advances HEAD linearly — producing exactly the
#      linear history AC-10 of 13-6 asks for.
#
# `packed-refs.lock`, `HEAD.lock` (different message: "Unable to create
# '.../HEAD.lock'"), and mkdir collisions are NOT in this set — they
# would indicate a stuck ref-packer, not a writer race, and retrying
# would just spin.
_LOCK_COLLISION_RE = re.compile(
    r"Unable to create [^\s]*index\.lock|cannot lock ref "
)

# Internal marker prepended to LibraryCommitError messages by `_git`
# when the underlying git stderr matched _LOCK_COLLISION_RE on the RAW
# (pre-sanitization) text. The marker survives through the NFR9 path
# sanitizer so `_is_lock_collision` can still detect the collision
# after the error message has crossed the sanitization boundary. The
# marker is a fixed ASCII token chosen to be obviously internal and to
# never appear in real git stderr.
_LOCK_COLLISION_MARKER = "[LIBRARY_LOCK_COLLISION] "


def _is_lock_collision(stderr: str) -> bool:
    """Pure classification: does this stderr come from an index-lock collision?

    Inspects the stderr text only — never the exception type — because
    :class:`LibraryCommitError` already wraps both lock and non-lock git
    failures. Used by :meth:`LibrarySandbox._git_with_lock_retry`.

    Two acceptance signals:
      1. The internal ``_LOCK_COLLISION_MARKER`` prefix added by
         :meth:`LibrarySandbox._git` when the *raw* stderr matched the
         lock-collision regex (production path).
      2. The raw lock-collision regex itself, for unit tests that
         construct a synthetic ``LibraryCommitError`` directly without
         going through ``_git``.
    """
    if not stderr:
        return False
    if _LOCK_COLLISION_MARKER in stderr:
        return True
    return _LOCK_COLLISION_RE.search(stderr) is not None

# Used by _sanitize_git_stderr to scrub absolute paths from git's stderr
# before they cross the LibraryCommitError boundary (NFR9 parity with
# PathEscapeError). Matches a leading "/" followed by any non-whitespace,
# non-colon run — i.e. "/workspace/.library/.git/index.lock" but NOT
# "fatal:" or "error:".
_ABS_PATH_RE = re.compile(r"/[^\s:]+")


def _sanitize_git_stderr(stderr: str) -> str:
    """Replace absolute filesystem paths in git stderr with ``<path>``.

    NFR9 forbids leaking raw filesystem paths into cloud logs. The git
    CLI freely emits absolute paths in failure messages (e.g.
    ``fatal: Unable to create '/workspace/.library/.git/index.lock'``),
    so we filter every match before wrapping in ``LibraryCommitError``.
    """
    return _ABS_PATH_RE.sub("<path>", stderr)


def _default_disk_space_check(git_root: str) -> None:
    """Default pre-commit disk-space check (NFR22).

    Uses :func:`shutil.disk_usage` against the git workdir and raises
    :class:`LibraryDiskFullError` when free bytes fall below
    :data:`_DEFAULT_MIN_FREE_BYTES`. Operators may inject a stricter
    check via the ``disk_space_check`` constructor parameter.
    """
    usage = shutil.disk_usage(git_root)
    if usage.free < _DEFAULT_MIN_FREE_BYTES:
        raise LibraryDiskFullError(required_bytes=_DEFAULT_MIN_FREE_BYTES)


@dataclasses.dataclass
class _Transaction:
    """In-memory bookkeeping for a single open transaction.

    ``staged_paths`` accumulates the relative paths touched by ``write()``
    and ``delete()`` calls during the block. The actual ``git add`` is
    deferred to ``commit()`` so a 500-page transaction runs one staging
    call instead of 500 (ADR-039: one commit per skill invocation, never
    per write).
    """

    operation: str
    summary: str
    staged_paths: list[str] = dataclasses.field(default_factory=list)


@dataclasses.dataclass
class TransactionHandle:
    """Handle yielded by :meth:`LibrarySandbox.transaction`.

    Skill code mutates :attr:`commit_metadata` during the transactional
    block to accumulate values that will become structured commit-message
    body fields (Sources, Pages created, Pages updated, Cross-references
    added). The handle deliberately exposes a plain ``dict`` so callers
    can append items as they discover them without needing to know which
    keys are well-known.
    """

    commit_metadata: dict[str, Any] = dataclasses.field(default_factory=dict)


def _hash_path(canonical: str) -> str:
    """Return a short hex digest of a canonical path for diagnostics only.

    Used by :class:`PathEscapeError` to correlate repeated escape attempts
    without ever emitting the raw path to logs (NFR9).
    """
    return hashlib.sha256(canonical.encode("utf-8", errors="replace")).hexdigest()[:12]


class LibrarySandbox:
    """Filesystem sandbox bound to a single Library root.

    Construction canonicalizes the provided ``library_root`` via
    :func:`os.path.realpath`, so passing a symlink resolves to the real
    target directory. The root must exist and must be a directory.

    Args:
        library_root: Filesystem path to the Library root. Must exist and
            be a directory.

    Raises:
        FileNotFoundError: if the resolved root does not exist.
        ValueError: if the resolved root is not a directory.
    """

    def __init__(
        self,
        library_root: PathLike,
        *,
        git_enabled: bool = False,
        agent_name: str = "unknown",
        pre_commit_hook: Callable[[list[str]], None] | None = None,
        disk_space_check: Callable[[str], None] | None = None,
        stale_lock_timeout_s: float = _DEFAULT_STALE_LOCK_TIMEOUT_S,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        resolved = os.path.realpath(os.fspath(library_root))
        if not os.path.exists(resolved):
            raise FileNotFoundError(
                f"Library root does not exist: {resolved}"
            )
        if not os.path.isdir(resolved):
            raise ValueError(f"Library root is not a directory: {resolved}")
        self._root: str = resolved
        # Git layer (Story 13-4). All four parameters are no-ops when
        # git_enabled is False — backward compatible with 13-1 callers.
        self._git_enabled: bool = git_enabled
        self._agent_name: str = agent_name
        self._pre_commit_hook: Callable[[list[str]], None] | None = pre_commit_hook
        self._disk_space_check: Callable[[str], None] = (
            disk_space_check if disk_space_check is not None else _default_disk_space_check
        )
        self._txn: _Transaction | None = None
        # Crash-recovery configuration (Story 13-5) and retry-loop sleep
        # seam (Story 13-6). Both fields have safe defaults so existing
        # 13-1 / 13-4 call sites are unaffected. The `sleep` param is
        # reused by 13-6's `_git_with_lock_retry` so unit tests can drive
        # the retry loop in microseconds instead of real wall-clock time.
        self._stale_lock_timeout_s: float = stale_lock_timeout_s
        self._sleep: Callable[[float], None] = sleep

    # ─── Public API ────────────────────────────────────────────────────

    def read(self, rel_path: str) -> str:
        """Read a UTF-8 text file inside the Library root."""
        target = self._validate(rel_path)
        with open(target, "r", encoding="utf-8") as f:
            return f.read()

    def read_bytes(self, rel_path: str) -> bytes:
        """Read a raw binary file inside the Library root.

        Mirrors :meth:`read` but returns ``bytes`` instead of a
        UTF-8-decoded ``str`` — used by the library_ingest PDF branch
        (story 13-9) where opening the file as text would silently
        corrupt the PDF binary stream.

        Path canonicalization and the ``PathEscapeError`` / ``ValueError``
        error semantics are identical to :meth:`read`. This helper does
        NOT stage a git change — it is a pure read-side accessor.
        """
        target = self._validate(rel_path)
        with open(target, "rb") as f:
            return f.read()

    def write(self, rel_path: str, content: str) -> None:
        """Write a UTF-8 text file inside the Library root.

        Parent directories are created as needed, but only *after* the
        path has been validated — an escape attempt never creates any
        directory on disk.
        """
        target = self._validate(rel_path)
        parent = os.path.dirname(target)
        if parent and not os.path.isdir(parent):
            os.makedirs(parent, exist_ok=True)
        with open(target, "w", encoding="utf-8") as f:
            f.write(content)
        self._record_staged(rel_path)

    def list(self, rel_path: str = ".") -> list[str]:
        """Recursively list file paths under ``rel_path`` (relative to root).

        Returns file paths relative to the Library root, sorted
        lexicographically. Directory entries are skipped — only files
        appear in the result. ``rel_path`` must resolve inside the root.
        """
        target = self._validate(rel_path)
        if not os.path.isdir(target):
            # Consistent with os.listdir: if the validated path exists but
            # is not a directory, raise NotADirectoryError.
            raise NotADirectoryError(f"Not a directory: {rel_path}")
        results: list[str] = []
        for dirpath, _dirnames, filenames in os.walk(target):
            for name in filenames:
                abs_path = os.path.join(dirpath, name)
                rel = os.path.relpath(abs_path, self._root)
                # Normalize to forward slashes for deterministic output on
                # all platforms (Library pages are logically POSIX paths).
                results.append(rel.replace(os.sep, "/"))
        results.sort()
        return results

    def delete(self, rel_path: str) -> None:
        """Remove a file inside the Library root."""
        target = self._validate(rel_path)
        os.remove(target)
        self._record_staged(rel_path)

    def exists(self, rel_path: str) -> bool:
        """Return True iff ``rel_path`` resolves to an existing path inside the root.

        Note: escape attempts still raise :class:`PathEscapeError` for
        consistency with the other methods — this method cannot be used
        to silently probe for paths outside the sandbox.
        """
        target = self._validate(rel_path)
        return os.path.exists(target)

    # ─── Internals ─────────────────────────────────────────────────────

    def _validate(self, rel_path: str) -> str:
        """Resolve ``rel_path`` against the root and enforce containment.

        Returns the canonical absolute path on success. Raises
        :class:`PathEscapeError` on any attempt to escape the root.
        Raises :class:`ValueError` on empty input.
        """
        if not isinstance(rel_path, str) or rel_path == "":
            # Empty path is nonsensical — reject structurally. This is
            # consistent with the rest of the API (no silent pass-through).
            raise ValueError("rel_path must be a non-empty string")

        # os.path.join handles absolute paths by discarding the prefix —
        # i.e. join("/root", "/etc/passwd") == "/etc/passwd". That is
        # intentional here: it means realpath will then canonicalize the
        # absolute path outside the root, and the containment check below
        # rejects it.
        candidate = os.path.realpath(os.path.join(self._root, rel_path))

        # Containment via commonpath — NOT startswith. A naive
        # startswith check would allow ``/tmp/libA-evil`` to bypass a
        # root of ``/tmp/libA`` (sibling-prefix attack).
        try:
            common = os.path.commonpath([candidate, self._root])
        except ValueError:
            # commonpath raises ValueError if the paths are on different
            # drives (Windows) or mix absolute/relative. Any such case is
            # an escape.
            raise PathEscapeError(debug_hash=_hash_path(candidate)) from None

        if common != self._root:
            raise PathEscapeError(debug_hash=_hash_path(candidate))

        return candidate

    # ─── Git transactional layer (Story 13-4) ──────────────────────────

    def _record_staged(self, rel_path: str) -> None:
        """Record a write/delete in the active transaction (no-op otherwise).

        The path is stored in insertion order with deduplication so that
        repeatedly writing the same file does not produce a noisy commit
        message. The actual ``git add`` is deferred to ``commit()``.
        """
        if self._txn is None:
            return
        # Normalize separators so list-comparisons in tests are stable
        # across platforms — Library pages are logically POSIX paths
        # (consistent with .list()).
        normalized = rel_path.replace(os.sep, "/")
        if normalized not in self._txn.staged_paths:
            self._txn.staged_paths.append(normalized)

    def _git(
        self,
        args: list[str],
        *,
        check: bool = True,
        suppress_stderr: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        """Single seam for every ``git`` invocation under the sandbox.

        Centralizing subprocess.run here keeps the test surface tiny:
        13-6 (concurrent-write serialization) will add lock-detection
        inside this method without touching the rest of the class.

        On non-zero exit (when ``check`` is True) the method raises
        :class:`LibraryCommitError` with a sanitized stderr (NFR9). The
        caller is responsible for any working-tree cleanup.
        """
        env = os.environ.copy()
        env["GIT_AUTHOR_NAME"] = f"Agent {self._agent_name}"
        env["GIT_AUTHOR_EMAIL"] = f"{self._agent_name}@wheelhouse.dev"
        env["GIT_COMMITTER_NAME"] = f"Agent {self._agent_name}"
        env["GIT_COMMITTER_EMAIL"] = f"{self._agent_name}@wheelhouse.dev"
        result = subprocess.run(  # noqa: S603 — fixed argv, no shell
            ["git", *args],
            cwd=self._root,
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=_GIT_SUBPROCESS_TIMEOUT_S,
        )
        if check and result.returncode != 0:
            raw_stderr = result.stderr or ""
            stderr = "" if suppress_stderr else _sanitize_git_stderr(raw_stderr)
            # Story 13-6: classify lock collisions on the RAW stderr
            # before path sanitization erases the `index.lock` token,
            # then prepend a stable marker so `_is_lock_collision` can
            # detect the collision after the message has crossed the
            # NFR9 sanitization boundary.
            if _LOCK_COLLISION_RE.search(raw_stderr):
                marker = _LOCK_COLLISION_MARKER
            else:
                marker = ""
            raise LibraryCommitError(
                f"{marker}git {args[0] if args else ''} failed: {stderr.strip() or '<no stderr>'}"
            )
        return result

    # ─── Story 13-6: index.lock retry layer (FR22, NFR18, ADR-040) ─────

    def _lock_path(self) -> str:
        """Return the absolute path of git's native index lock file.

        Single seam so a test can swap it. Production callers should
        treat the result as opaque — only `_lock_age_seconds` and
        `_remove_stale_lock_if_present` interpret it.
        """
        return os.path.join(self._root, _INDEX_LOCK_REL)

    def _lock_age_seconds(self) -> float | None:
        """Return the age in seconds of `.git/index.lock`, or None if absent.

        Returns ``None`` if the lock does not exist (a competing writer
        already released it). Catches :class:`FileNotFoundError` to make
        the stat-then-act sequence race-tolerant (AC-7).
        """
        try:
            st = os.stat(self._lock_path())
        except FileNotFoundError:
            return None
        return time.time() - st.st_mtime

    def _remove_stale_lock_if_present(self) -> bool:
        """Remove `.git/index.lock` iff it is older than the stale window.

        Returns ``True`` when a stale lock was successfully cleaned up
        (or vanished concurrently — both count as "the lock is no longer
        in our way"); ``False`` otherwise. The warning log line never
        embeds the absolute lock path, per NFR9.
        """
        age = self._lock_age_seconds()
        if age is None:
            # Lock vanished between collision and stat — fine, retry.
            return True
        if age <= _LOCK_STALE_AFTER_S:
            return False
        try:
            os.unlink(self._lock_path())
        except FileNotFoundError:
            # Race: another agent removed it first. Still success.
            pass
        logger.warning("stale Library lock removed (age=%.1fs)", age)
        return True

    def _git_with_lock_retry(
        self, args: list[str]
    ) -> subprocess.CompletedProcess[str]:
        """Wrap a mutating git call with the ADR-040 retry protocol.

        Routing matrix:

        - **Mutating** git calls (``add``, ``commit``, ``reset``,
          ``clean``, ``rm --cached``) MUST go through this wrapper.
        - **Read-only** git calls (``rev-parse --verify HEAD``) MUST
          stay on the bare :meth:`_git` path — they never touch
          ``index.lock`` and routing them here would needlessly add a
          try-frame (AC-6).

        Behaviour per ADR-040 §"Normal behaviour":

        1. Try the call. On success, return.
        2. On a non-lock failure, propagate unchanged (AC-9).
        3. On a lock collision, attempt one stale-lock removal — if it
           succeeds, retry immediately (does not consume an attempt).
        4. Otherwise wait ``_LOCK_RETRY_WAIT_S`` and retry, up to
           ``_LOCK_MAX_ATTEMPTS`` fresh-wait attempts.
        5. After all attempts, raise :class:`LibraryBusyError`.

        The stale-lock retry is capped at one per call so a chronically
        stuck lock cannot loop forever. Stale removal grants a single
        bonus retry that does not count toward the fresh-wait budget,
        because once we have removed a stale lock the very next git
        call should succeed if no other writer is racing us.

        On :class:`LibraryBusyError` the original git stderr is dropped
        — the busy error is the actionable signal and stderr never
        leaves this function (NFR9, AC-8).
        """
        stale_removed = False
        attempt = 0
        while True:
            try:
                return self._git(args)
            except LibraryCommitError as exc:
                if not _is_lock_collision(str(exc)):
                    raise
                if self._remove_stale_lock_if_present():
                    if stale_removed:
                        # Already used our one stale-removal grace.
                        raise LibraryBusyError() from None
                    stale_removed = True
                    continue
                attempt += 1
                if attempt >= _LOCK_MAX_ATTEMPTS:
                    raise LibraryBusyError() from None
                self._sleep(_LOCK_RETRY_WAIT_S)
                continue

    def _ensure_git_repo(self) -> None:
        """Idempotently initialize ``.git/`` under the Library root.

        No-op when ``git_enabled`` is False, so 13-1 callers never see a
        ``.git/`` directory created behind their back. Called lazily from
        ``begin()`` / ``commit()`` — never from ``__init__`` — to keep
        the no-git path bit-for-bit compatible with the 13-1 baseline.
        """
        if not self._git_enabled:
            return
        if os.path.isdir(os.path.join(self._root, ".git")):
            return
        # Story 13-6: `git init` cannot collide on `.git/index.lock`
        # because the index file does not exist yet, and `git config
        # --local` only writes `.git/config`. These calls intentionally
        # bypass the lock-retry wrapper.
        self._git(["init", "-q"])
        # Deterministic line endings + no GPG dependency. These are
        # local-only so they cannot pollute the operator's global git
        # config.
        self._git(["config", "--local", "core.autocrlf", "false"])
        self._git(["config", "--local", "commit.gpgsign", "false"])

    def _has_head(self) -> bool:
        """Return True iff the repo has at least one commit on HEAD.

        Read-only — never goes through ``_git_with_lock_retry``.
        ``git rev-parse --verify HEAD`` only reads ``.git/HEAD`` and
        ``.git/refs/`` and does not create ``index.lock``, so it cannot
        race against a concurrent writer (story 13-6, AC-6, ADR-040).
        """
        result = self._git(
            ["rev-parse", "--verify", "HEAD"],
            check=False,
            suppress_stderr=True,
        )
        return result.returncode == 0

    def begin(self, operation: str, summary: str) -> None:
        """Open a transactional block.

        Validates the structured-commit invariants up front (subject
        line is ``[<operation>] <summary>``, must be a single line of
        non-empty text) so a malformed message is rejected at the start
        of the block instead of after the skill has done its work.

        Raises:
            LibraryTransactionError: if ``git_enabled`` is False or a
                transaction is already open (no nesting).
            ValueError: if ``operation`` or ``summary`` is empty or
                contains whitespace that would corrupt the subject line.
        """
        if not self._git_enabled:
            raise LibraryTransactionError(
                "begin() requires git_enabled=True"
            )
        if self._txn is not None:
            raise LibraryTransactionError(
                "Nested transactions are not supported"
            )
        if not operation or not operation.strip() or any(c in operation for c in " \t\n\r"):
            raise ValueError(
                "operation must be a non-empty single token without whitespace"
            )
        if not summary or "\n" in summary or "\r" in summary:
            raise ValueError(
                "summary must be a non-empty single line"
            )
        self._ensure_git_repo()
        self._txn = _Transaction(operation=operation, summary=summary, staged_paths=[])

    def commit(
        self,
        *,
        sources: list[str] | None = None,
        pages_created: list[str] | None = None,
        pages_updated: list[str] | None = None,
        cross_references_added: int | None = None,
    ) -> None:
        """Stage and commit every path touched in the active transaction.

        Performs the full pre-commit pipeline:

        1. Snapshot and clear ``self._txn`` (so a failure cannot leave a
           dangling transaction).
        2. ``git add -- <staged_paths>`` (single call; ``git add`` stages
           creations and deletions alike since git ≥ 2.0).
        3. ``self._disk_space_check(self._root)`` — NFR22.
        4. ``self._pre_commit_hook(staged_paths)`` if injected — ADR-039
           cross-reference hook point.
        5. Build the structured commit message per ADR-039 and run
           ``git commit -m <message>``.

        On any failure between steps 2 and 5 the working tree is reset
        (via ``rollback()``) so there is no half-committed state (NFR19).

        Raises:
            LibraryTransactionError: no active transaction or
                ``git_enabled`` is False.
            LibraryDiskFullError: pre-commit disk-space check failed.
            LibraryCommitError: ``git add`` / ``git commit`` failed.
            Exception: any exception raised by ``pre_commit_hook`` is
                propagated unchanged after the working tree is reset.
        """
        if not self._git_enabled:
            raise LibraryTransactionError(
                "commit() requires git_enabled=True"
            )
        if self._txn is None:
            raise LibraryTransactionError(
                "commit() called without an active transaction"
            )

        # Snapshot and clear the transaction BEFORE any git call. This
        # guarantees that a failed commit cannot leave a dangling
        # self._txn — the rollback path below sees a clean slate and
        # can run git reset directly without re-entering this function.
        txn = self._txn
        self._txn = None
        staged_paths = list(txn.staged_paths)

        try:
            if staged_paths:
                # Story 13-6: routed through the lock-retry wrapper
                # because `git add` is the call that creates index.lock.
                self._git_with_lock_retry(["add", "--", *staged_paths])
            # Pre-commit checks. Either of these aborting must leave
            # the working tree clean — handled by the except block.
            self._disk_space_check(self._root)
            if self._pre_commit_hook is not None:
                self._pre_commit_hook(list(staged_paths))

            message = self._build_commit_message(
                operation=txn.operation,
                summary=txn.summary,
                sources=sources,
                pages_created=pages_created,
                pages_updated=pages_updated,
                cross_references_added=cross_references_added,
            )
            # Story 13-6: `git commit` re-acquires index.lock briefly to
            # write the new tree, so it goes through the retry wrapper too.
            self._git_with_lock_retry(["commit", "-m", message, "--allow-empty"])
        except BaseException:
            # Reset the working tree to a clean state and re-raise the
            # original exception unchanged. We use a fresh _Transaction
            # snapshot so _rollback_internal can clean exactly the paths
            # touched in this aborted block.
            self._rollback_internal(staged_paths)
            raise

    def rollback(self) -> None:
        """Discard the active transaction's working-tree changes.

        Idempotent: a no-op if no transaction is open.

        Raises:
            LibraryTransactionError: ``git_enabled`` is False.
        """
        if not self._git_enabled:
            raise LibraryTransactionError(
                "rollback() requires git_enabled=True"
            )
        if self._txn is None:
            return
        txn = self._txn
        self._txn = None
        self._rollback_internal(list(txn.staged_paths))

    def _try_git_for_rollback(self, args: list[str]) -> None:
        """Best-effort mutating git call used only by rollback paths.

        Story 13-6: rollback runs after a primary failure has already
        decided the call site's outcome. We still want to wait on
        ``index.lock`` so the cleanup actually completes when another
        writer is briefly racing us, but we MUST swallow any final
        :class:`LibraryGitError` so the primary exception (which the
        caller is in the middle of re-raising) reaches them unchanged.
        """
        try:
            self._git_with_lock_retry(args)
        except LibraryCommitError:
            pass
        except LibraryBusyError:
            pass

    def _rollback_internal(self, staged_paths: list[str]) -> None:
        """Reset the working tree, branching on whether HEAD exists.

        On a fresh ``git init``-ed repo there is no HEAD, so
        ``git reset --hard HEAD`` would fail with
        ``fatal: ambiguous argument 'HEAD'``. We detect that case via
        ``git rev-parse --verify HEAD`` and fall back to deleting the
        untracked files individually — they are by definition the only
        thing the transaction could have produced.
        """
        if not staged_paths:
            return
        if self._has_head():
            # Reset previously-committed files (handles modifications
            # and deletions of tracked files in this txn). Story 13-6:
            # routed through the lock-aware best-effort wrapper so a
            # racing writer cannot strand our half-staged state, while
            # the primary exception still surfaces unchanged.
            self._try_git_for_rollback(["reset", "--hard", "HEAD"])
            # Reset alone cannot remove brand-new untracked files. Use
            # git clean scoped to the staged paths to remove them.
            self._try_git_for_rollback(["clean", "-fd", "--", *staged_paths])
        else:
            # No HEAD yet → nothing to reset to. The transaction may
            # have already added paths to the index (if `git add` ran
            # before the failure point), so we must clear those index
            # entries before deleting the working-tree files. Without
            # this, an aborted-then-retried first commit would carry
            # over orphaned index entries from the failed run.
            self._try_git_for_rollback(
                ["rm", "-rf", "--cached", "--ignore-unmatch", "--", *staged_paths]
            )
            for rel in staged_paths:
                abs_path = os.path.join(self._root, rel)
                try:
                    if os.path.isdir(abs_path):
                        shutil.rmtree(abs_path, ignore_errors=True)
                    else:
                        os.remove(abs_path)
                except FileNotFoundError:
                    pass

    @contextlib.contextmanager
    def transaction(self, operation: str, summary: str) -> Iterator[TransactionHandle]:
        """Context-manager wrapper around ``begin()`` / ``commit()``.

        Yields a :class:`TransactionHandle` whose ``commit_metadata``
        dict the block body can mutate to accumulate Sources, Pages
        created, etc. On normal exit those values are passed to
        ``commit()``; on exception the transaction is rolled back and
        the exception is re-raised unchanged.
        """
        self.begin(operation, summary)
        handle = TransactionHandle()
        try:
            yield handle
        except BaseException:
            # rollback() is a no-op if commit() already cleared self._txn,
            # but the common path here is that the block raised before
            # commit() was reached.
            self.rollback()
            raise
        self.commit(**handle.commit_metadata)

    # ─── Crash recovery (Story 13-5) ───────────────────────────────────

    def recover_from_crash(self) -> None:
        """Boot-time crash-recovery sweep — safe to call before any skill runs.

        Implements the ADR-040 recovery sequence:

        1. Remove or wait-out a stale ``.library/.git/index.lock`` left by
           a ``git`` invocation that never returned (NFR18).
        2. Discard any uncommitted modifications and untracked files in
           the working tree (NFR19, NFR23) so the next ingest/lint starts
           from a clean baseline.

        The method is **idempotent** (a no-op on a clean repo), is **safe
        to call before any transaction** (does not touch ``self._txn``),
        and is a **complete no-op when ``git_enabled`` is False** even if
        a ``.git/`` directory happens to exist under the Library root.

        The agent boot path (Story 13-7 / startup.py) is responsible for
        calling this exactly once at startup. This module ships only the
        method and its tests.
        """
        # Step 1 — git_enabled gate. Absolute: never inspect the FS or
        # call git when git is disabled, even if .git/ exists.
        if not self._git_enabled:
            return

        # Step 2 — no .git/ yet (sandbox never used) → nothing to recover.
        if not os.path.isdir(os.path.join(self._root, ".git")):
            return

        # Step 3 — handle stale .git/index.lock (NFR18, ADR-040).
        self._recover_index_lock()

        # Step 4 — discard uncommitted working-tree changes (NFR19/NFR23).
        self._recover_working_tree()

    def _recover_index_lock(self) -> None:
        """Remove ``.library/.git/index.lock`` if stale, else wait then remove.

        TOCTOU-robust: another process removing the lock between our
        ``stat()`` and ``unlink()`` calls is treated as success (the
        post-condition "lock not present" is already met).
        """
        lock_path = os.path.join(self._root, ".git", "index.lock")
        if not os.path.exists(lock_path):
            return
        try:
            st = os.stat(lock_path)
        except FileNotFoundError:
            return  # AC-11 race: lock disappeared between exists() and stat()

        age = max(time.time() - st.st_mtime, 0.0)
        if age >= self._stale_lock_timeout_s:
            self._unlink_lock_quietly(lock_path)
            _LOGGER.warning(
                "Removed stale Library .git/index.lock (age: %.1fs)",
                age,
            )
            return

        # Fresh lock — wait the remaining budget and then remove.
        remaining = self._stale_lock_timeout_s - age
        self._sleep(remaining)
        self._unlink_lock_quietly(lock_path)
        _LOGGER.warning(
            "Waited %.1fs for Library .git/index.lock and then removed it",
            remaining,
        )

    @staticmethod
    def _unlink_lock_quietly(lock_path: str) -> None:
        """``os.unlink`` that swallows ``FileNotFoundError`` (TOCTOU race)."""
        try:
            os.unlink(lock_path)
        except FileNotFoundError:
            return  # AC-11: another process won the unlink race — fine.

    def _recover_working_tree(self) -> None:
        """Discard uncommitted modifications and untracked files at boot.

        Uses ``git status --porcelain`` to detect dirty state. On a dirty
        tree:

        - With HEAD: ``git reset --hard HEAD`` reverts tracked-file
          modifications, then ``git clean -fd`` removes untracked files.
          A warning is logged with the short HEAD hash.
        - Without HEAD (fresh repo): ``git read-tree --empty`` clears any
          orphaned index entries from a partial ``git add`` before the
          crash, then ``git clean -fd`` removes the working-tree files.
          A warning is logged noting "no commits yet".

        Repo-scoped (not path-scoped) because at boot we have no list of
        what the crashed process was writing — the only safe option is
        "clean everything not tracked or ignored". Runtime rollback
        (Story 13-4) is path-scoped because it knows the active
        transaction's staged paths.
        """
        status = self._git(["status", "--porcelain"])
        if not (status.stdout or "").strip():
            return  # AC-1: clean tree → no warning, no work.

        if self._has_head():
            short_hash_result = self._git(["rev-parse", "--short", "HEAD"])
            short_hash = (short_hash_result.stdout or "").strip() or "<unknown>"
            self._git(["reset", "--hard", "HEAD"])
            self._git(["clean", "-fd"], check=False)
            _LOGGER.warning(
                "Discarded uncommitted Library changes from prior crash (reset to %s)",
                short_hash,
            )
        else:
            # No HEAD yet — clear index then wipe untracked working tree.
            self._git(["read-tree", "--empty"], check=False)
            self._git(["clean", "-fd"], check=False)
            _LOGGER.warning(
                "Discarded uncommitted Library changes from prior crash (no commits yet)"
            )

    @staticmethod
    def _build_commit_message(
        *,
        operation: str,
        summary: str,
        sources: list[str] | None,
        pages_created: list[str] | None,
        pages_updated: list[str] | None,
        cross_references_added: int | None,
    ) -> str:
        """Render the structured commit message defined by ADR-039.

        Format:

            [<operation>] <summary>

            Sources: <comma-separated>
            Pages created: <comma-separated>
            Pages updated: <comma-separated>
            Cross-references added: <N>

        Empty/None body fields are omitted entirely (no
        ``Sources:`` line with nothing after the colon). Downstream
        tooling (wh-cli status, audit, activity feed) parses this
        format directly, so it is a de-facto API — see ADR-039.
        """
        subject = f"[{operation}] {summary}"
        body_lines: list[str] = []
        if sources:
            body_lines.append(f"Sources: {', '.join(sources)}")
        if pages_created:
            body_lines.append(f"Pages created: {', '.join(pages_created)}")
        if pages_updated:
            body_lines.append(f"Pages updated: {', '.join(pages_updated)}")
        if cross_references_added is not None:
            body_lines.append(f"Cross-references added: {cross_references_added}")
        if body_lines:
            return subject + "\n\n" + "\n".join(body_lines)
        return subject
