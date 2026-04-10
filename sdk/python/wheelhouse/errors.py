"""Wheelhouse SDK error types (MA-02).

Callers import errors directly:
    from wheelhouse.errors import ConnectionError, PublishTimeout, StreamNotFound
"""


class WheelhouseError(Exception):
    """Base class for all Wheelhouse SDK errors."""

    def __init__(self, message: str, code: str | None = None):
        super().__init__(message)
        self.code = code


class ConnectionError(WheelhouseError):  # noqa: A001 — intentionally shadows builtin per MA-02
    """Wheelhouse is not running or unreachable.

    User-facing: never says 'broker' or 'connection refused' (RT-B1).

    Note: This intentionally shadows Python's builtin ConnectionError per the
    architecture spec (MA-02). Import as `from wheelhouse.errors import ConnectionError`
    or use the qualified name `wheelhouse.errors.ConnectionError`.
    """

    pass


class PublishTimeout(WheelhouseError):
    """publish_confirmed() timed out waiting for WAL acknowledgement (SCV-08)."""

    def __init__(
        self,
        message: str | None = None,
        *,
        stream: str | None = None,
        timeout: float | None = None,
        code: str | None = "PUBLISH_TIMEOUT",
    ):
        if message is None:
            parts = []
            if stream:
                parts.append(f"stream '{stream}'")
            if timeout is not None:
                parts.append(f"timeout {timeout}s")
            message = "Publish timed out" + (f": {', '.join(parts)}" if parts else "")
        super().__init__(message, code=code)


class StreamNotFound(WheelhouseError):
    """Requested stream does not exist."""

    pass


class RegistrationError(WheelhouseError):
    """Type registration was rejected by Wheelhouse."""

    pass


class ReservedNamespaceError(RegistrationError):
    """Attempted to register a type under the reserved 'wheelhouse.*' namespace (ADR-004)."""

    pass


class InvalidTypeNameError(RegistrationError):
    """Type name does not match required format '<namespace>.<TypeName>'."""

    pass


class RegistryFullError(RegistrationError):
    """Type registry has reached its capacity limit (RT-05)."""

    pass


class LibraryGitError(WheelhouseError):
    """Base class for Library git-backed persistence failures (Story 13-4).

    The Library stores every skill invocation as a single structured git
    commit (ADR-039). Failures along the commit path — staging, pre-commit
    checks, the ``git commit`` invocation itself, transactional misuse —
    surface as subclasses of this error so callers can ``except
    LibraryGitError`` once and handle the entire family.
    """

    pass


class LibraryCommitError(LibraryGitError):
    """A ``git`` invocation underneath :class:`LibrarySandbox` failed.

    Raised when ``git add`` / ``git commit`` / ``git reset`` exits non-zero
    or otherwise misbehaves. The message is sanitized: NFR9 (parity with
    :class:`PathEscapeError`) requires that absolute filesystem paths from
    git's stderr never leak into cloud logs, so the sandbox replaces any
    path-looking token in stderr with ``<path>`` before wrapping it.
    """

    pass


class LibraryDiskFullError(LibraryGitError):
    """The Library volume is out of space (NFR22).

    Raised by the pre-commit disk-space check before any ``git commit`` is
    attempted, so the working tree and index are guaranteed not to be
    half-mutated. The message is a fixed actionable prefix so operators
    and end-users can recognise and respond to it consistently.
    """

    _MESSAGE_PREFIX = "Library is full — free disk space to continue writing"

    def __init__(
        self,
        message: str | None = None,
        *,
        required_bytes: int | None = None,
        code: str = "LIBRARY_DISK_FULL",
    ) -> None:
        super().__init__(message or self._MESSAGE_PREFIX, code=code)
        self.required_bytes = required_bytes


class LibraryBusyError(LibraryGitError):
    """Concurrent Library write contention exhausted retries (FR22, ADR-040).

    Raised by :class:`LibrarySandbox` when a mutating git invocation
    repeatedly fails to acquire ``.library/.git/index.lock`` because
    another writer is holding it. Per ADR-040 the sandbox waits 2s and
    retries up to 3 attempts before giving up; if a stale lock (>5min)
    is detected it is removed once and the call retried, otherwise this
    error surfaces.

    The message is a fixed actionable prefix and embeds **no** filesystem
    paths so the error is safe to forward into cloud logs (NFR9).
    """

    _MESSAGE_PREFIX = "Library is busy — another write operation is in progress"

    def __init__(
        self,
        message: str | None = None,
        *,
        code: str = "LIBRARY_BUSY",
    ) -> None:
        super().__init__(message or self._MESSAGE_PREFIX, code=code)


class LibraryTransactionError(LibraryGitError):
    """A LibrarySandbox transactional API was used incorrectly.

    Raised for misuse like calling ``commit()`` without an active
    transaction, calling ``commit()`` / ``rollback()`` when
    ``git_enabled=False``, or opening a nested transaction.
    """

    pass


class PathEscapeError(WheelhouseError):
    """Filesystem path escapes the Library sandbox root (FR38, NFR7, NFR9).

    Raised by ``wheelhouse.skills.library_sandbox.LibrarySandbox`` when a
    caller attempts to read/write/list/delete a path that, after
    canonicalization (``os.path.realpath``), does not reside under the
    Library root.

    NFR9 requires that the attempted raw path never leak into cloud logs.
    To enforce this structurally, the public message is a fixed generic
    string and never embeds the attempted path. For local correlation of
    repeated escape attempts, a short sha256 prefix of the canonicalized
    attempted path is exposed via the ``_debug_hash`` attribute — this is
    a **diagnostic-only** attribute, not a public API contract.
    """

    _GENERIC_MESSAGE = "Path blocked outside Library root"

    def __init__(self, *, debug_hash: str = "", code: str = "PATH_ESCAPE") -> None:
        super().__init__(self._GENERIC_MESSAGE, code=code)
        # Underscore prefix signals: diagnostic-only, not a stable public API.
        self._debug_hash = debug_hash


# ──────────────────────────────────────────────────────────────────────
# Library skill-layer error contract (Story 13-7)
# ──────────────────────────────────────────────────────────────────────
#
# Distinct from :class:`LibraryGitError` on purpose. The LibraryGitError
# family covers git-layer failures from inside ``LibrarySandbox`` itself
# (commit failed, disk full, busy lock, transactional misuse).
# ``LibrarySkillError`` covers SKILL-layer failures surfaced by the
# invocation handlers in ``wheelhouse.skills.library_ingest`` and (later)
# ``wheelhouse.skills.library_lint`` / ``library_retrieval``: argument
# validation, plan-state refusals (read-only), disabled-mode refusals,
# and the scaffold-sentinel "pipeline not yet implemented" marker used by
# 13-7 while 13-8 / 13-9 are still in flight.
#
# The ``code`` attribute is the stable contract — ``SkillResult.error_code``
# carries it verbatim into the wh-cloud billing/observability pipeline.
# 13-7 freezes four codes; 13-8..13-13 will ADD codes but must not rename
# or repurpose the ones defined here.


class LibrarySkillError(WheelhouseError):
    """Structured failure raised by a Library skill invocation handler.

    Parent of the code catalogue consumed by ``SkillResult.error_code``.
    Distinct from :class:`LibraryGitError` because skill-layer failures
    (bad args, read-only plan, disabled Library, not-yet-implemented
    scaffolding) are not git failures — they must not be caught by an
    ``except LibraryGitError:`` handler inside sandbox internals.

    The ``code`` argument is mandatory — every call site must name the
    stable code string it is producing, which becomes the
    ``SkillResult.error_code`` surfaced to cloud. See
    ``wheelhouse.skills.library_ingest`` for the initial catalogue.
    """

    def __init__(self, message: str, *, code: str) -> None:
        super().__init__(message, code=code)
