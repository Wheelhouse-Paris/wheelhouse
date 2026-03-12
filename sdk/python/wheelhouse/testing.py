"""Test utilities for Wheelhouse SDK.

Usage:
    from wheelhouse.testing import MockConnection

This module has an import guard: ZMQ is NOT imported at module level,
so it can be used in environments without ZMQ installed.
"""

from __future__ import annotations

from typing import Any, Awaitable, Callable


class MockConnection:
    """A mock connection for testing surfaces and handlers without a running Wheelhouse.

    Records all published messages and allows simulating incoming messages
    to subscriptions.

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
        self._connected = True

    async def publish(self, stream: str, message: Any) -> None:
        """Record a published message.

        Args:
            stream: Target stream name.
            message: The message object.
        """
        self.published.append((stream, message))

    async def publish_confirmed(
        self, stream: str, message: Any, timeout: float = 5.0
    ) -> None:
        """Record a confirmed publish.

        Args:
            stream: Target stream name.
            message: The message object.
            timeout: Timeout value (recorded but not enforced in mock).
        """
        self.confirmed.append((stream, message, timeout))

    async def subscribe(
        self, stream: str, handler: Callable[[Any], Awaitable[None]]
    ) -> None:
        """Register a subscription handler.

        Args:
            stream: Stream name.
            handler: Async handler function.
        """
        if stream not in self._subscriptions:
            self._subscriptions[stream] = []
        self._subscriptions[stream].append(handler)

    async def simulate_message(self, stream: str, message: Any) -> None:
        """Simulate an incoming message to all handlers subscribed to a stream.

        Args:
            stream: The stream the message arrives on.
            message: The message to deliver to handlers.
        """
        handlers = self._subscriptions.get(stream, [])
        for handler in handlers:
            await handler(message)

    def register_type(self, type_name: str, type_class: type) -> None:
        """Register a custom type (for mock compatibility)."""
        self._registered_types[type_name] = type_class

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
