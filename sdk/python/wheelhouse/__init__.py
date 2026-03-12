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
