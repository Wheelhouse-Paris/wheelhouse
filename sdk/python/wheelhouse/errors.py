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
