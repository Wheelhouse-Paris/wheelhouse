"""Wheelhouse test utilities — MockConnection for development without Podman.

Import guard: this module is separate from __init__.py (CF-07).
"""

from __future__ import annotations

from typing import Any

from wheelhouse.types import TypedMessage


class MockConnection:
    """Mock connection for testing without a running Wheelhouse instance.

    Messages published in mock mode are echoed back to subscribers
    registered in the same mock session (NFR-D4).
    """

    def __init__(self) -> None:
        self._registered_types: dict[str, type] = {}
        self._messages: list[TypedMessage] = []

    def register_type(self, type_name: str, type_class: type) -> None:
        """Register a type in the mock registry."""
        self._registered_types[type_name] = type_class

    def publish(self, stream_name: str, type_name: str, data: Any) -> None:
        """Publish a message to the mock — echoed back to subscribers."""
        self._messages.append(TypedMessage.known(type_name, data))

    def get_messages(self) -> list[TypedMessage]:
        """Get all messages published in this mock session."""
        return list(self._messages)

    def clear(self) -> None:
        """Clear all mock state."""
        self._messages.clear()
