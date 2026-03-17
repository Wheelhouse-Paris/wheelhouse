"""Tests for batch_publisher module (Story 10.4, ADR-022).

Tests cover:
  - Multi-stream publishing
  - reply_to_user_id routing (source stream only)
  - Publish failure handling (continues remaining items)
  - Empty list (no publish)
"""

from __future__ import annotations

import logging
from unittest.mock import AsyncMock, call

import pytest

from wheelhouse.types import TextMessage

from agent_claude.batch_publisher import publish_batch


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def mock_connection() -> AsyncMock:
    """Create a mock connection with async publish."""
    conn = AsyncMock()
    conn.publish = AsyncMock()
    return conn


# ---------------------------------------------------------------------------
# Multi-stream publish tests (AC-1)
# ---------------------------------------------------------------------------


class TestPublishBatch:
    """Tests for batch publishing."""

    @pytest.mark.asyncio
    async def test_publishes_to_multiple_streams(
        self, mock_connection: AsyncMock
    ) -> None:
        """AC-1: Publishes each item to its target stream."""
        items = [
            {"stream": "main", "type": "TextMessage", "content": "Hello main"},
            {"stream": "logs", "type": "TextMessage", "content": "Logged"},
        ]
        await publish_batch(
            mock_connection, items, "agent-1", source_stream="main"
        )
        assert mock_connection.publish.call_count == 2

        # Verify streams
        calls = mock_connection.publish.call_args_list
        assert calls[0][0][0] == "main"
        assert calls[1][0][0] == "logs"

        # Verify content
        msg1 = calls[0][0][1]
        msg2 = calls[1][0][1]
        assert isinstance(msg1, TextMessage)
        assert isinstance(msg2, TextMessage)
        assert msg1.content == "Hello main"
        assert msg2.content == "Logged"
        assert msg1.publisher_id == "agent-1"
        assert msg2.publisher_id == "agent-1"

    @pytest.mark.asyncio
    async def test_reply_to_user_id_source_stream_only(
        self, mock_connection: AsyncMock
    ) -> None:
        """AC-7: reply_to_user_id set only for source stream, not cross-stream."""
        items = [
            {"stream": "main", "type": "TextMessage", "content": "Reply"},
            {"stream": "logs", "type": "TextMessage", "content": "Cross"},
        ]
        await publish_batch(
            mock_connection,
            items,
            "agent-1",
            source_stream="main",
            reply_to_user_id="user-42",
        )

        calls = mock_connection.publish.call_args_list
        msg_main = calls[0][0][1]
        msg_logs = calls[1][0][1]

        # Source stream gets reply_to_user_id
        assert msg_main.reply_to_user_id == "user-42"
        # Cross-stream does NOT get reply_to_user_id
        assert msg_logs.reply_to_user_id in (None, "")

    @pytest.mark.asyncio
    async def test_no_reply_to_user_id_when_none(
        self, mock_connection: AsyncMock
    ) -> None:
        """AC-7: When reply_to_user_id is None, no message gets it."""
        items = [
            {"stream": "main", "type": "TextMessage", "content": "Hello"},
        ]
        await publish_batch(
            mock_connection, items, "agent-1", source_stream="main",
            reply_to_user_id=None,
        )
        calls = mock_connection.publish.call_args_list
        msg = calls[0][0][1]
        assert msg.reply_to_user_id in (None, "")

    @pytest.mark.asyncio
    async def test_publish_failure_continues(
        self, mock_connection: AsyncMock, caplog: pytest.LogCaptureFixture
    ) -> None:
        """Publish failure on one item does not abort remaining items."""
        mock_connection.publish = AsyncMock(
            side_effect=[Exception("network error"), None]
        )
        items = [
            {"stream": "bad", "type": "TextMessage", "content": "Fail"},
            {"stream": "good", "type": "TextMessage", "content": "OK"},
        ]
        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            await publish_batch(
                mock_connection, items, "agent-1", source_stream="main"
            )

        # Both publishes attempted
        assert mock_connection.publish.call_count == 2
        # Warning logged for failure
        assert any("Failed to publish batch item" in r.message for r in caplog.records)

    @pytest.mark.asyncio
    async def test_empty_list_no_publish(
        self, mock_connection: AsyncMock
    ) -> None:
        """AC-2: Empty list results in no publish calls."""
        await publish_batch(
            mock_connection, [], "agent-1", source_stream="main"
        )
        mock_connection.publish.assert_not_called()

    @pytest.mark.asyncio
    async def test_single_item_source_stream(
        self, mock_connection: AsyncMock
    ) -> None:
        """AC-3: Single item targeting source stream = backward compatible."""
        items = [
            {"stream": "main", "type": "TextMessage", "content": "Reply"},
        ]
        await publish_batch(
            mock_connection,
            items,
            "agent-1",
            source_stream="main",
            reply_to_user_id="user-1",
        )
        assert mock_connection.publish.call_count == 1
        msg = mock_connection.publish.call_args[0][1]
        assert msg.content == "Reply"
        assert msg.publisher_id == "agent-1"
        assert msg.reply_to_user_id == "user-1"
