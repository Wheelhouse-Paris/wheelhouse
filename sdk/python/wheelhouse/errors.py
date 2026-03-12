"""Wheelhouse SDK error types.

Import errors directly:
    from wheelhouse.errors import ConnectionError, PublishTimeout, StreamNotFound
"""


class ConnectionError(Exception):
    """Raised when the SDK cannot connect to Wheelhouse.

    The error message includes the endpoint address and a human-readable cause.
    Never exposes internal details (ZMQ, socket types, port numbers).
    """

    def __init__(self, endpoint: str, cause: str) -> None:
        self.endpoint = endpoint
        self.cause = cause
        super().__init__(
            f"Could not connect to Wheelhouse at {endpoint}: {cause}"
        )


class PublishTimeout(Exception):
    """Raised when publish_confirmed() does not receive acknowledgement within the timeout."""

    def __init__(self, stream: str, timeout: float) -> None:
        self.stream = stream
        self.timeout = timeout
        super().__init__(
            f"Publish to stream '{stream}' was not confirmed within {timeout}s"
        )


class StreamNotFound(Exception):
    """Raised when the specified stream does not exist."""

    def __init__(self, stream: str) -> None:
        self.stream = stream
        super().__init__(f"Stream '{stream}' not found")
