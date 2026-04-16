"""Tests for LibraryWriteEvent emission (Story 14.1.1, AC-4, AC-6)."""

from __future__ import annotations

import os
from unittest.mock import AsyncMock, patch

import pytest

from wheelhouse.librarian.emit import maybe_emit_write_event
from wheelhouse._proto.wheelhouse.librarian.v1 import LibraryWriteEvent


@pytest.fixture
def mock_connection():
    """Provide a mock connection with async publish."""
    conn = AsyncMock()
    conn.publish = AsyncMock()
    return conn


@pytest.mark.asyncio
async def test_no_emit_without_env_var(mock_connection):
    """AC-6: No publish when WH_LIBRARY_WRITE_STREAM is not set."""
    with patch.dict(os.environ, {}, clear=True):
        # Ensure the env vars are absent
        os.environ.pop("WH_LIBRARY_WRITE_STREAM", None)
        os.environ.pop("WH_LIBRARY_ID", None)

        await maybe_emit_write_event(
            connection=mock_connection,
            agent_name="test-agent",
            user_message="hello",
            assistant_response="hi there",
            conversation_id="conv-1",
        )

    mock_connection.publish.assert_not_called()


@pytest.mark.asyncio
async def test_no_emit_without_library_id(mock_connection):
    """AC-6: No publish when WH_LIBRARY_ID is missing."""
    with patch.dict(os.environ, {"WH_LIBRARY_WRITE_STREAM": "eot-agent-lib"}, clear=True):
        os.environ.pop("WH_LIBRARY_ID", None)

        await maybe_emit_write_event(
            connection=mock_connection,
            agent_name="test-agent",
            user_message="hello",
            assistant_response="hi there",
            conversation_id="conv-1",
        )

    mock_connection.publish.assert_not_called()


@pytest.mark.asyncio
async def test_emit_when_stream_configured(mock_connection):
    """AC-4: Emits LibraryWriteEvent when both env vars are set."""
    env = {
        "WH_LIBRARY_WRITE_STREAM": "eot-myagent-research",
        "WH_LIBRARY_ID": "research",
    }
    with patch.dict(os.environ, env, clear=True):
        await maybe_emit_write_event(
            connection=mock_connection,
            agent_name="myagent",
            user_message="What is Rust?",
            assistant_response="Rust is a systems programming language.",
            conversation_id="conv-123",
        )

    mock_connection.publish.assert_called_once()
    call_args = mock_connection.publish.call_args
    stream_name = call_args[0][0]
    event = call_args[0][1]

    assert stream_name == "eot-myagent-research"
    assert isinstance(event, LibraryWriteEvent)


@pytest.mark.asyncio
async def test_event_contains_required_fields(mock_connection):
    """AC-4: Event has event_id (UUID), source_agent_id, timestamp_ms > 0."""
    env = {
        "WH_LIBRARY_WRITE_STREAM": "eot-agent-lib",
        "WH_LIBRARY_ID": "lib",
    }
    with patch.dict(os.environ, env, clear=True):
        await maybe_emit_write_event(
            connection=mock_connection,
            agent_name="agent-a",
            user_message="Hello",
            assistant_response="Hi",
            conversation_id="conv-1",
        )

    event = mock_connection.publish.call_args[0][1]

    # event_id is a UUID string
    assert len(event.event_id) == 36  # UUID format: 8-4-4-4-12
    assert "-" in event.event_id

    assert event.source_agent_id == "agent-a"
    assert event.library_id == "lib"
    assert event.conversation_id == "conv-1"
    assert event.timestamp_ms > 0


@pytest.mark.asyncio
async def test_event_segment_populated(mock_connection):
    """AC-4: Conversation segment contains user and assistant messages."""
    env = {
        "WH_LIBRARY_WRITE_STREAM": "eot-agent-lib",
        "WH_LIBRARY_ID": "lib",
    }
    with patch.dict(os.environ, env, clear=True):
        await maybe_emit_write_event(
            connection=mock_connection,
            agent_name="agent-a",
            user_message="What is Python?",
            assistant_response="A programming language.",
            conversation_id="conv-1",
        )

    event = mock_connection.publish.call_args[0][1]

    assert len(event.segment) == 2
    assert event.segment[0].role == "user"
    assert event.segment[0].content == "What is Python?"
    assert event.segment[1].role == "assistant"
    assert event.segment[1].content == "A programming language."


@pytest.mark.asyncio
async def test_publish_failure_does_not_raise(mock_connection):
    """Emission failure is logged but never raises (fire-and-forget)."""
    mock_connection.publish.side_effect = Exception("ZMQ error")

    env = {
        "WH_LIBRARY_WRITE_STREAM": "eot-agent-lib",
        "WH_LIBRARY_ID": "lib",
    }
    with patch.dict(os.environ, env, clear=True):
        # Should not raise
        await maybe_emit_write_event(
            connection=mock_connection,
            agent_name="agent-a",
            user_message="hello",
            assistant_response="hi",
            conversation_id="conv-1",
        )
