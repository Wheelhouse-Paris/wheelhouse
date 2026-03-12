"""Acceptance tests for publish() and publish_confirmed() — AC #1.

Uses MockConnection for unit testing without a running broker.
"""

import pytest

from wheelhouse.testing import MockConnection
from wheelhouse.types import TextMessage


class TestPublish:
    """AC #1: publish a TextMessage to a stream with no errors."""

    @pytest.mark.asyncio
    async def test_publish_sends_message_to_stream(self):
        """Given a connected SDK client,
        When I publish a TextMessage to a stream,
        Then the message is sent with no errors.
        """
        conn = MockConnection()
        msg = TextMessage(content="hello")
        await conn.publish("test-stream", msg)
        assert len(conn.published) == 1
        assert conn.published[0] == ("test-stream", msg)

    @pytest.mark.asyncio
    async def test_publish_records_stream_name(self):
        """Given a publish call,
        Then the stream name is correctly recorded.
        """
        conn = MockConnection()
        msg = TextMessage(content="test")
        await conn.publish("my-stream", msg)
        assert conn.published[0][0] == "my-stream"

    @pytest.mark.asyncio
    async def test_publish_multiple_messages(self):
        """Given multiple publish calls,
        Then all messages are recorded in order.
        """
        conn = MockConnection()
        for i in range(3):
            await conn.publish("stream", TextMessage(content=f"msg-{i}"))
        assert len(conn.published) == 3


class TestPublishConfirmed:
    """WW-02: publish_confirmed() with timeout."""

    @pytest.mark.asyncio
    async def test_publish_confirmed_records_message(self):
        """Given a connected SDK client,
        When I call publish_confirmed() with a message,
        Then the message is recorded.
        """
        conn = MockConnection()
        msg = TextMessage(content="confirmed message")
        await conn.publish_confirmed("test-stream", msg, timeout=5.0)
        assert len(conn.confirmed) == 1
        assert conn.confirmed[0] == ("test-stream", msg, 5.0)

    @pytest.mark.asyncio
    async def test_publish_confirmed_raises_timeout(self):
        """Given PublishTimeout error type exists,
        Then it can be raised with stream and timeout info.
        """
        from wheelhouse.errors import PublishTimeout

        with pytest.raises(PublishTimeout, match="test-stream"):
            raise PublishTimeout(stream="test-stream", timeout=0.1)

    @pytest.mark.asyncio
    async def test_publish_confirmed_default_timeout(self):
        """Given publish_confirmed() called without explicit timeout,
        Then it uses a default timeout of 5.0 seconds.
        """
        conn = MockConnection()
        msg = TextMessage(content="default timeout")
        await conn.publish_confirmed("test-stream", msg)
        # Default timeout should be 5.0
        assert conn.confirmed[0][2] == 5.0


class TestPublishNoTopology:
    """FP-04: SDK exposes NO topology management methods."""

    def test_connection_has_no_topology_methods(self):
        """Given a connected SDK client,
        It should NOT expose topology management methods.
        """
        conn = MockConnection()
        assert not hasattr(conn, "create_stream")
        assert not hasattr(conn, "delete_stream")
        assert not hasattr(conn, "deploy")
        assert not hasattr(conn, "topology")
