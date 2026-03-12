"""Acceptance tests for Story 6.3: Test and Mock Mode — Development Without Podman.

These tests verify that mock mode provides a complete testing experience
without requiring a running Wheelhouse instance or Podman installation.

Acceptance Criteria:
  AC1: connect(mock=True) echoes published messages to subscribers, no ZMQ used
  AC2: pytest-based testing works without Podman, message assertions pass
  AC3: Switching from mock to real requires only connection call change
  AC4: @register_type validates schema in mock mode — not a no-op
"""

import asyncio
import subprocess
import sys
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).parent.parent.parent


# ---------------------------------------------------------------------------
# AC1: Mock Mode Echo — No ZMQ
# ---------------------------------------------------------------------------

class TestMockModeEcho:
    """connect(mock=True) must echo published messages to subscribers with no ZMQ."""

    @pytest.mark.asyncio
    async def test_mock_publish_echoes_to_subscriber(self):
        """Given connect(mock=True),
        When I publish a message,
        Then the message is echoed to subscribers in the same session.
        """
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        received = []

        async def handler(msg):
            received.append(msg)

        await conn.subscribe("test-stream", handler)
        await conn.publish("test-stream", TextMessage(content="hello mock"))

        assert len(received) == 1
        assert received[0].content == "hello mock"
        await conn.close()

    @pytest.mark.asyncio
    async def test_mock_does_not_use_zmq(self):
        """Given connect(mock=True),
        Then the MockConnection module does not import zmq.
        """
        import wheelhouse.testing
        import sys

        # zmq should not be imported as a dependency of wheelhouse.testing
        # (CF-07 import guard)
        testing_module = sys.modules["wheelhouse.testing"]
        source = Path(testing_module.__file__).read_text()
        assert "import zmq" not in source, (
            "wheelhouse.testing must not import zmq (NFR-D4 / CF-07)"
        )

    @pytest.mark.asyncio
    async def test_mock_multiple_subscribers_receive(self):
        """Given multiple subscribers on the same stream,
        When a message is published in mock mode,
        Then all subscribers receive the message.
        """
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        received_a = []
        received_b = []

        async def handler_a(msg):
            received_a.append(msg)

        async def handler_b(msg):
            received_b.append(msg)

        await conn.subscribe("stream", handler_a)
        await conn.subscribe("stream", handler_b)
        await conn.publish("stream", TextMessage(content="broadcast"))

        assert len(received_a) == 1
        assert len(received_b) == 1
        assert received_a[0].content == "broadcast"
        await conn.close()

    @pytest.mark.asyncio
    async def test_mock_different_streams_isolated(self):
        """Given subscribers on different streams,
        When a message is published to one stream,
        Then only that stream's subscribers receive it.
        """
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        received_a = []
        received_b = []

        async def handler_a(msg):
            received_a.append(msg)

        async def handler_b(msg):
            received_b.append(msg)

        await conn.subscribe("stream-a", handler_a)
        await conn.subscribe("stream-b", handler_b)
        await conn.publish("stream-a", TextMessage(content="only-a"))

        assert len(received_a) == 1
        assert len(received_b) == 0
        await conn.close()

    @pytest.mark.asyncio
    async def test_mock_publish_confirmed_also_echoes(self):
        """Given mock mode, publish_confirmed also echoes to subscribers."""
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        received = []

        async def handler(msg):
            received.append(msg)

        await conn.subscribe("stream", handler)
        await conn.publish_confirmed("stream", TextMessage(content="confirmed"))

        assert len(received) == 1
        assert received[0].content == "confirmed"
        await conn.close()


# ---------------------------------------------------------------------------
# AC2: Pytest Testing Without Podman
# ---------------------------------------------------------------------------

class TestPytestWithoutPodman:
    """pytest tests with mock mode must work without Podman installed."""

    @pytest.mark.asyncio
    async def test_message_content_assertion(self):
        """Given mock mode,
        When I publish and subscribe,
        Then I can assert on the received message content.
        """
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        received = []

        async def handler(msg):
            received.append(msg)

        await conn.subscribe("test", handler)
        msg = TextMessage(content="assertion test", user_id="user-1")
        await conn.publish("test", msg)

        assert len(received) == 1
        assert received[0].content == "assertion test"
        assert received[0].user_id == "user-1"
        await conn.close()

    @pytest.mark.asyncio
    async def test_mock_connection_context_manager(self):
        """Given mock mode with async with,
        When the context exits,
        Then the connection is closed.
        """
        import wheelhouse

        async with await wheelhouse.connect(mock=True) as conn:
            assert conn._connected is True
        assert conn._connected is False

    @pytest.mark.asyncio
    async def test_mock_get_messages_for_assertions(self):
        """Given mock mode,
        When I publish multiple messages,
        Then get_messages() returns all published messages for assertion.
        """
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        await conn.publish("s1", TextMessage(content="first"))
        await conn.publish("s2", TextMessage(content="second"))

        messages = conn.get_messages()
        assert len(messages) == 2
        assert messages[0].data.content == "first"
        assert messages[1].data.content == "second"
        await conn.close()

    @pytest.mark.asyncio
    async def test_mock_published_list(self):
        """Given mock mode,
        When I publish messages,
        Then conn.published records all (stream, message) tuples.
        """
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        await conn.publish("stream-a", TextMessage(content="a"))
        await conn.publish("stream-b", TextMessage(content="b"))

        assert len(conn.published) == 2
        assert conn.published[0][0] == "stream-a"
        assert conn.published[1][0] == "stream-b"
        await conn.close()

    @pytest.mark.asyncio
    async def test_fixtures_module_exists(self):
        """Fixtures module must exist for pytest convenience."""
        from wheelhouse.fixtures import mock_connection
        assert callable(mock_connection)

    @pytest.mark.asyncio
    async def test_assert_published_helper(self):
        """MockConnection.assert_published() must filter by stream and count."""
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        await conn.publish("orders", TextMessage(content="order-1"))
        await conn.publish("orders", TextMessage(content="order-2"))
        await conn.publish("logs", TextMessage(content="log-1"))

        orders = conn.get_published("orders")
        assert len(orders) == 2

        logs = conn.get_published("logs")
        assert len(logs) == 1
        await conn.close()

    @pytest.mark.asyncio
    async def test_reset_clears_state(self):
        """MockConnection.reset() clears all recorded messages."""
        import wheelhouse
        from wheelhouse.types import TextMessage

        conn = await wheelhouse.connect(mock=True)
        await conn.publish("s", TextMessage(content="x"))
        assert len(conn.published) == 1

        conn.reset()
        assert len(conn.published) == 0
        assert len(conn.get_messages()) == 0
        await conn.close()


# ---------------------------------------------------------------------------
# AC3: API Parity — Mock ↔ Real Switching
# ---------------------------------------------------------------------------

class TestAPIParity:
    """Switching from mock=True to mock=False requires only the connection call change."""

    def test_mock_connection_has_publish(self):
        """MockConnection has publish() with same signature as Connection."""
        from wheelhouse.testing import MockConnection
        from wheelhouse._core import Connection
        import inspect

        mock_sig = inspect.signature(MockConnection.publish)
        real_sig = inspect.signature(Connection.publish)
        # Both should accept (self, stream: str, message: Any)
        assert list(mock_sig.parameters.keys()) == list(real_sig.parameters.keys())

    def test_mock_connection_has_subscribe(self):
        """MockConnection has subscribe() with same signature as Connection."""
        from wheelhouse.testing import MockConnection
        from wheelhouse._core import Connection
        import inspect

        mock_sig = inspect.signature(MockConnection.subscribe)
        real_sig = inspect.signature(Connection.subscribe)
        assert list(mock_sig.parameters.keys()) == list(real_sig.parameters.keys())

    def test_mock_connection_has_publish_confirmed(self):
        """MockConnection has publish_confirmed() with same signature as Connection."""
        from wheelhouse.testing import MockConnection
        from wheelhouse._core import Connection
        import inspect

        mock_sig = inspect.signature(MockConnection.publish_confirmed)
        real_sig = inspect.signature(Connection.publish_confirmed)
        assert list(mock_sig.parameters.keys()) == list(real_sig.parameters.keys())

    def test_mock_connection_has_close(self):
        """MockConnection has close()."""
        from wheelhouse.testing import MockConnection
        conn = MockConnection()
        assert hasattr(conn, "close")
        assert callable(conn.close)

    def test_mock_connection_has_context_manager(self):
        """MockConnection supports async with."""
        from wheelhouse.testing import MockConnection
        assert hasattr(MockConnection, "__aenter__")
        assert hasattr(MockConnection, "__aexit__")

    def test_mock_connection_has_register_type(self):
        """MockConnection has register_type() method."""
        from wheelhouse.testing import MockConnection
        conn = MockConnection()
        assert hasattr(conn, "register_type")
        assert callable(conn.register_type)


# ---------------------------------------------------------------------------
# AC4: Schema Validation in Mock Mode — Not a No-Op
# ---------------------------------------------------------------------------

class TestSchemaValidationInMockMode:
    """@register_type must validate type schema even in mock mode."""

    def test_register_type_rejects_empty_class(self):
        """Given an empty class with no fields,
        When @register_type is applied,
        Then TypeError is raised with a descriptive message.
        """
        import wheelhouse

        with pytest.raises(TypeError, match="field|attribute"):
            @wheelhouse.register_type("test.EmptyType")
            class EmptyType:
                pass

    def test_register_type_accepts_dataclass_with_fields(self):
        """Given a dataclass with fields,
        When @register_type is applied,
        Then registration succeeds without error.
        """
        import wheelhouse
        from dataclasses import dataclass

        @wheelhouse.register_type("test.ValidType")
        @dataclass
        class ValidType:
            name: str = ""
            value: int = 0

        assert hasattr(ValidType, "_wh_type_name")
        assert ValidType._wh_type_name == "test.ValidType"

    def test_register_type_accepts_class_with_annotations(self):
        """Given a class with type annotations (not a dataclass),
        When @register_type is applied,
        Then registration succeeds.
        """
        import wheelhouse

        @wheelhouse.register_type("test.AnnotatedType")
        class AnnotatedType:
            name: str
            value: int

        assert hasattr(AnnotatedType, "_wh_type_name")

    def test_register_type_rejects_no_annotations_class(self):
        """Given a class with methods but no data fields,
        When @register_type is applied,
        Then TypeError is raised.
        """
        import wheelhouse

        with pytest.raises(TypeError):
            @wheelhouse.register_type("test.MethodOnly")
            class MethodOnly:
                def do_something(self):
                    pass

    def test_schema_validation_same_in_mock_and_real(self):
        """Schema validation runs at decoration time — same regardless of mode.
        The @register_type decorator validates immediately, before connect().
        """
        import wheelhouse

        # This should fail before any connect() call
        with pytest.raises(TypeError):
            @wheelhouse.register_type("test.AlsoEmpty")
            class AlsoEmpty:
                pass


# ---------------------------------------------------------------------------
# Example 04 — Testing Example
# ---------------------------------------------------------------------------

class TestExample4Testing:
    """Example 4 demonstrates pytest patterns with mock mode."""

    def test_example_4_exists(self):
        """Example 4 file must exist at examples/04_testing.py."""
        example_path = PROJECT_ROOT / "examples" / "04_testing.py"
        assert example_path.exists(), f"Expected {example_path} to exist"

    def test_example_4_runs_without_error(self):
        """Example 4 runs and exits with code 0."""
        example_path = PROJECT_ROOT / "examples" / "04_testing.py"
        result = subprocess.run(
            [sys.executable, str(example_path)],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert result.returncode == 0, f"Exit code {result.returncode}, stderr: {result.stderr}"

    def test_example_4_uses_mock_mode(self):
        """Example 4 must demonstrate mock mode for testing."""
        example_path = PROJECT_ROOT / "examples" / "04_testing.py"
        content = example_path.read_text()
        assert "mock" in content.lower(), "Example 4 must demonstrate mock mode"
