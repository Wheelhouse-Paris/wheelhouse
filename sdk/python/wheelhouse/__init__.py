"""Wheelhouse Python SDK — connect, publish, and subscribe to streams.

Usage:
    import wheelhouse

    conn = await wheelhouse.connect()
    await conn.publish("my-stream", TextMessage(content="hello"))
    await conn.close()

Types: from wheelhouse.types import TextMessage
Errors: from wheelhouse.errors import ConnectionError, PublishTimeout
Testing: from wheelhouse.testing import MockConnection
"""

from __future__ import annotations

from typing import Any, Awaitable, Callable

from wheelhouse._core import connect as connect  # noqa: F401

# Restrict public API surface
__all__ = ["connect", "Surface"]


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
        """Initialize with a Wheelhouse connection.

        Args:
            connection: A Connection object from wheelhouse.connect().
        """
        self._connection = connection

    async def publish(self, stream: str, message: Any) -> None:
        """Publish a message to a stream.

        Args:
            stream: Target stream name.
            message: A Protobuf-compatible message object.
        """
        await self._connection.publish(stream, message)

    async def publish_confirmed(
        self, stream: str, message: Any, timeout: float = 5.0
    ) -> None:
        """Publish a message and wait for confirmation.

        Args:
            stream: Target stream name.
            message: A Protobuf-compatible message object.
            timeout: Maximum seconds to wait. Defaults to 5.0.

        Raises:
            wheelhouse.errors.PublishTimeout: If not confirmed within timeout.
        """
        await self._connection.publish_confirmed(stream, message, timeout=timeout)

    async def subscribe(
        self, stream: str, handler: Callable[[Any], Awaitable[None]]
    ) -> None:
        """Subscribe to a stream with a handler.

        Args:
            stream: Stream name to subscribe to.
            handler: Async function called with each deserialized message.
        """
        await self._connection.subscribe(stream, handler)

    async def on_message(self, message: Any) -> None:
        """Override this to handle incoming messages.

        Args:
            message: The deserialized message object.
        """
        pass

    async def on_connect(self) -> None:
        """Override this for custom logic on connection establishment."""
        pass
