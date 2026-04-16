"""Tests for LibraryWriteEvent protobuf definitions (Story 14.1.1, AC-1, AC-2)."""

from __future__ import annotations

import betterproto


def test_import_from_types():
    """AC-2: LibraryWriteEvent and ConversationMessage importable from wheelhouse.types."""
    from wheelhouse.types import ConversationMessage, LibraryWriteEvent

    assert LibraryWriteEvent is not None
    assert ConversationMessage is not None


def test_import_from_proto():
    """AC-2: Direct import from proto package."""
    from wheelhouse._proto.wheelhouse.librarian.v1 import (
        ConversationMessage,
        LibraryWriteEvent,
    )

    assert LibraryWriteEvent is not None
    assert ConversationMessage is not None


def test_library_write_event_fields():
    """AC-1: LibraryWriteEvent has all required fields with correct types."""
    from wheelhouse.types import LibraryWriteEvent

    evt = LibraryWriteEvent(
        event_id="evt-001",
        source_agent_id="agent-a",
        library_id="research",
        conversation_id="conv-123",
        timestamp_ms=1710000000000,
        locale="en",
    )
    assert evt.event_id == "evt-001"
    assert evt.source_agent_id == "agent-a"
    assert evt.library_id == "research"
    assert evt.conversation_id == "conv-123"
    assert evt.timestamp_ms == 1710000000000
    assert evt.locale == "en"
    assert evt.segment == []
    assert evt.metadata == {}


def test_conversation_message_fields():
    """AC-1: ConversationMessage has role, content, timestamp_ms."""
    from wheelhouse.types import ConversationMessage

    msg = ConversationMessage(
        role="user",
        content="Hello",
        timestamp_ms=1710000000000,
    )
    assert msg.role == "user"
    assert msg.content == "Hello"
    assert msg.timestamp_ms == 1710000000000


def test_library_write_event_roundtrip():
    """AC-1: Serialize/deserialize roundtrip for LibraryWriteEvent."""
    from wheelhouse.types import ConversationMessage, LibraryWriteEvent

    original = LibraryWriteEvent(
        event_id="evt-roundtrip",
        source_agent_id="agent-test",
        library_id="lab",
        conversation_id="conv-rt",
        timestamp_ms=1710000000000,
        segment=[
            ConversationMessage(role="user", content="What is Python?", timestamp_ms=1710000000000),
            ConversationMessage(role="assistant", content="A programming language.", timestamp_ms=1710000001000),
        ],
        locale="en",
        metadata={"source": "test"},
    )

    encoded = bytes(original)
    decoded = LibraryWriteEvent().parse(encoded)

    assert decoded.event_id == original.event_id
    assert decoded.source_agent_id == original.source_agent_id
    assert decoded.library_id == original.library_id
    assert decoded.conversation_id == original.conversation_id
    assert decoded.timestamp_ms == original.timestamp_ms
    assert decoded.locale == original.locale
    assert len(decoded.segment) == 2
    assert decoded.segment[0].role == "user"
    assert decoded.segment[0].content == "What is Python?"
    assert decoded.segment[1].role == "assistant"
    assert decoded.metadata["source"] == "test"


def test_conversation_message_roundtrip():
    """AC-1: Serialize/deserialize roundtrip for ConversationMessage."""
    from wheelhouse.types import ConversationMessage

    original = ConversationMessage(
        role="assistant",
        content="Bonjour!",
        timestamp_ms=1710000000000,
    )

    encoded = bytes(original)
    decoded = ConversationMessage().parse(encoded)

    assert decoded.role == original.role
    assert decoded.content == original.content
    assert decoded.timestamp_ms == original.timestamp_ms


def test_library_write_event_default_values():
    """Default LibraryWriteEvent has empty fields."""
    from wheelhouse.types import LibraryWriteEvent

    evt = LibraryWriteEvent()
    assert evt.event_id == ""
    assert evt.source_agent_id == ""
    assert evt.library_id == ""
    assert evt.locale == ""
    assert evt.segment == []
    assert evt.timestamp_ms == 0


def test_library_write_event_is_betterproto_message():
    """LibraryWriteEvent is a betterproto.Message subclass."""
    from wheelhouse.types import LibraryWriteEvent

    assert issubclass(LibraryWriteEvent, betterproto.Message)
