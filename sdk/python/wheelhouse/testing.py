"""Wheelhouse test utilities — MockConnection for development without a running Wheelhouse.

Import guard: ZMQ is NOT imported at module level,
so it can be used in environments without ZMQ installed (CF-07).
"""

from __future__ import annotations

from typing import Any, Awaitable, Callable

from wheelhouse.types import TypedMessage


class MockConnection:
    """A mock connection for testing surfaces and handlers without a running Wheelhouse.

    Records all published messages and allows simulating incoming messages
    to subscriptions. Supports both async (publish/subscribe) and sync
    (get_messages) access patterns.

    Example:
        mock = MockConnection()
        surface = MySurface(mock)
        await surface.publish("stream", TextMessage(content="test"))
        assert len(mock.published) == 1
    """

    def __init__(self) -> None:
        self.published: list[tuple[str, Any]] = []
        self.confirmed: list[tuple[str, Any, float]] = []
        self._subscriptions: dict[str, list[Callable[[Any], Awaitable[None]]]] = {}
        self._registered_types: dict[str, type] = {}
        self._messages: list[TypedMessage] = []
        self._connected = True

    async def publish(self, stream: str, message: Any) -> None:
        """Record a published message."""
        self.published.append((stream, message))
        type_name = type(message).__name__
        self._messages.append(TypedMessage.known(type_name, message))

    async def publish_confirmed(
        self, stream: str, message: Any, timeout: float = 5.0
    ) -> None:
        """Record a confirmed publish."""
        self.confirmed.append((stream, message, timeout))
        type_name = type(message).__name__
        self._messages.append(TypedMessage.known(type_name, message))

    async def subscribe(
        self, stream: str, handler: Callable[[Any], Awaitable[None]]
    ) -> None:
        """Register a subscription handler."""
        if stream not in self._subscriptions:
            self._subscriptions[stream] = []
        self._subscriptions[stream].append(handler)

    async def simulate_message(self, stream: str, message: Any) -> None:
        """Simulate an incoming message to all handlers subscribed to a stream."""
        handlers = self._subscriptions.get(stream, [])
        for handler in handlers:
            await handler(message)

    def register_type(self, type_name: str, type_class: type) -> None:
        """Register a custom type (for mock compatibility)."""
        self._registered_types[type_name] = type_class

    def get_messages(self) -> list[TypedMessage]:
        """Get all messages published in this mock session."""
        return list(self._messages)

    def clear(self) -> None:
        """Clear all mock state."""
        self.published.clear()
        self.confirmed.clear()
        self._messages.clear()

    async def close(self) -> None:
        """Close the mock connection."""
        self._connected = False
        self._subscriptions.clear()

    async def __aenter__(self) -> MockConnection:
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.close()
