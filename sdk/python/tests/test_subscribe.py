"""Acceptance tests for subscribe() — AC #2.

Uses MockConnection for unit testing without a running broker.
"""

import pytest

from wheelhouse.testing import MockConnection
from wheelhouse.types import TextMessage


class TestSubscribe:
    """AC #2: subscriber receives deserialized Protobuf objects."""

    @pytest.mark.asyncio
    async def test_subscribe_registers_handler(self):
        """Given a connected SDK client,
        When I call subscribe() with a stream and handler,
        Then the handler is registered for that stream.
        """
        conn = MockConnection()
        received = []

        async def handler(msg):
            received.append(msg)

        await conn.subscribe("test-stream", handler)
        assert "test-stream" in conn._subscriptions
        assert len(conn._subscriptions["test-stream"]) == 1

    @pytest.mark.asyncio
    async def test_subscribe_handler_receives_deserialized_object(self):
        """Given a subscriber is registered with a handler,
        When a matching message is published to the stream,
        Then the handler is called with the deserialized Protobuf object.
        """
        conn = MockConnection()
        received = []

        async def handler(msg):
            received.append(msg)

        await conn.subscribe("test-stream", handler)

        # Simulate an incoming message
        msg = TextMessage(content="hello subscriber")
        await conn.simulate_message("test-stream", msg)

        assert len(received) == 1
        assert isinstance(received[0], TextMessage)
        assert received[0].content == "hello subscriber"

    @pytest.mark.asyncio
    async def test_subscribe_multiple_handlers(self):
        """Given multiple subscribers on the same stream,
        When a message is published,
        Then all handlers are called.
        """
        conn = MockConnection()
        received_a = []
        received_b = []

        async def handler_a(msg):
            received_a.append(msg)

        async def handler_b(msg):
            received_b.append(msg)

        await conn.subscribe("test-stream", handler_a)
        await conn.subscribe("test-stream", handler_b)

        msg = TextMessage(content="multi handler")
        await conn.simulate_message("test-stream", msg)

        assert len(received_a) == 1
        assert len(received_b) == 1

    @pytest.mark.asyncio
    async def test_subscribe_different_streams(self):
        """Given handlers on different streams,
        When a message arrives on one stream,
        Then only that stream's handlers are called.
        """
        conn = MockConnection()
        received_a = []
        received_b = []

        async def handler_a(msg):
            received_a.append(msg)

        async def handler_b(msg):
            received_b.append(msg)

        await conn.subscribe("stream-a", handler_a)
        await conn.subscribe("stream-b", handler_b)

        msg = TextMessage(content="only for a")
        await conn.simulate_message("stream-a", msg)

        assert len(received_a) == 1
        assert len(received_b) == 0
