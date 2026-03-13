"""Wheelhouse Python SDK — connect, publish, and subscribe to streams.

Usage:
    import wheelhouse

    conn = await wheelhouse.connect()
    await conn.publish("my-stream", TextMessage(content="hello"))
    await conn.close()

Types: from wheelhouse.types import TextMessage
Errors: from wheelhouse.errors import ConnectionError, PublishTimeout
Testing: from wheelhouse.testing import MockConnection
Custom types: @wheelhouse.register_type("myapp.MyType")
"""

from __future__ import annotations

from typing import Any, Awaitable, Callable

__version__ = "0.1.0"

# _core (and zmq) are NOT imported at module level so that wheelhouse.testing
# can be imported without zmq installed (CF-07 import guard).

# Re-export user-facing error types for convenient `except wheelhouse.PublishTimeout` (AC #4)
from wheelhouse.errors import ConnectionError as ConnectionError  # noqa: A004, F401
from wheelhouse.errors import PublishTimeout as PublishTimeout  # noqa: F401
from wheelhouse.errors import StreamNotFound as StreamNotFound  # noqa: F401

# Restrict public API surface
__all__ = [
    "connect",
    "Surface",
    "register_type",
    "ConnectionError",
    "PublishTimeout",
    "StreamNotFound",
]


async def connect(
    endpoint: str | None = None,
    *,
    mock: bool = False,
    on_connection_event: Callable[[dict[str, Any]], None] | None = None,
) -> Any:  # Returns Connection | MockConnection
    """Connect to Wheelhouse.

    Args:
        endpoint: Wheelhouse endpoint URL. If not provided, uses WH_URL
                  environment variable, or defaults to tcp://127.0.0.1:5555.
        mock: If True, returns a MockConnection for testing without a running
              Wheelhouse instance (NFR-D4).
        on_connection_event: Optional callback for connection lifecycle events
            (CM-02). Receives a dict with "type" key: "disconnected",
            "reconnecting", "reconnected", or "reconnect_failed".

    Returns:
        A Connection (or MockConnection) for publishing and subscribing.

    Raises:
        wheelhouse.ConnectionError: If Wheelhouse is not running (real mode only).
    """
    if mock:
        from wheelhouse.testing import MockConnection
        return MockConnection()
    from wheelhouse._core import connect as _real_connect
    return await _real_connect(endpoint, on_connection_event=on_connection_event)


def register_type(type_name: str) -> Any:
    """Register a custom Protobuf type with a namespace.

    Lazy wrapper — imports _core (and zmq) only when called (CF-07).
    """
    from wheelhouse._core import register_type as _register_type

    return _register_type(type_name)


class Surface:
    """Base class for building custom Wheelhouse surfaces.

    Subclass this to create surfaces (CLI, Telegram, custom UI, etc.)
    that interact with streams via a Connection.

    Example:
        class MyCLISurface(Surface):
            async def on_message(self, message):
                print(message.content)
    """

    def __init__(self, connection: Any) -> None:
        self._connection = connection

    async def publish(self, stream: str, message: Any) -> None:
        await self._connection.publish(stream, message)

    async def publish_confirmed(
        self, stream: str, message: Any, timeout: float = 5.0
    ) -> None:
        await self._connection.publish_confirmed(stream, message, timeout=timeout)

    async def subscribe(
        self, stream: str, handler: Callable[[Any], Awaitable[None]]
    ) -> None:
        await self._connection.subscribe(stream, handler)

    async def on_message(self, message: Any) -> None:
        pass

    async def on_connect(self) -> None:
        pass
