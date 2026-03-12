"""Acceptance tests for auto-reconnect — Story 1.5.

Unit tests verifying reconnect backoff logic and connection event callbacks
without a running Wheelhouse instance.
"""

import pytest


class TestAutoReconnect:
    """AC #1, #3: automatic reconnect with exponential backoff."""

    def test_reconnect_capability_exists(self):
        """Given the Connection class,
        Then it has reconnect capability.
        """
        from wheelhouse._core import Connection

        conn = Connection("tcp://127.0.0.1:5555")
        assert hasattr(conn, "_reconnect")
        assert hasattr(conn, "_subscriptions")

    def test_reconnect_uses_exponential_backoff(self):
        """Given a connection drop,
        When the SDK reconnects,
        Then it uses exponential backoff: min(5s, 100ms * 2^attempt) + jitter.
        """
        from wheelhouse._core import _calculate_backoff

        # attempt 0: 100ms base + up to 100ms jitter
        backoff_0 = _calculate_backoff(0)
        assert 0.1 <= backoff_0 <= 0.2

        # attempt 1: 200ms base + up to 100ms jitter
        backoff_1 = _calculate_backoff(1)
        assert 0.2 <= backoff_1 <= 0.3

        # attempt 2: 400ms base + up to 100ms jitter
        backoff_2 = _calculate_backoff(2)
        assert 0.4 <= backoff_2 <= 0.5

        # attempt 10: capped at 5s + up to 100ms jitter
        backoff_cap = _calculate_backoff(10)
        assert 5.0 <= backoff_cap <= 5.1

    def test_connection_tracks_subscriptions(self):
        """Given active subscriptions exist when connection drops,
        The connection tracks subscriptions for re-registration (CM-07).
        """
        from wheelhouse._core import Connection

        conn = Connection("tcp://127.0.0.1:5555")
        assert hasattr(conn, "_subscriptions")
        assert isinstance(conn._subscriptions, dict)

    def test_connection_tracks_registered_types(self):
        """Given custom types were registered before disconnect,
        The connection tracks types for re-registration (CM-07).
        """
        from wheelhouse._core import Connection

        conn = Connection("tcp://127.0.0.1:5555")
        assert hasattr(conn, "_instance_types")
        assert isinstance(conn._instance_types, dict)

    def test_register_type(self):
        """Given a connection,
        When register_type() is called,
        Then the type is tracked for reconnect re-registration.
        """
        from wheelhouse._core import Connection
        from wheelhouse.types import TextMessage

        conn = Connection("tcp://127.0.0.1:5555")
        conn.register_type("TextMessage", TextMessage)
        assert "TextMessage" in conn._instance_types
        assert conn._instance_types["TextMessage"] is TextMessage


class TestConnectionEventCallback:
    """AC #3: Connection events surfaced to callback (CM-02)."""

    def test_connection_accepts_on_connection_event_callback(self):
        """Given a Connection,
        When on_connection_event callback is provided,
        Then the connection stores it for use during reconnect.
        """
        from wheelhouse._core import Connection

        events = []
        conn = Connection(
            "tcp://127.0.0.1:5555",
            on_connection_event=lambda e: events.append(e),
        )
        assert conn._on_connection_event is not None

    def test_connection_default_callback_is_none(self):
        """Given a Connection without callback,
        Then _on_connection_event is None.
        """
        from wheelhouse._core import Connection

        conn = Connection("tcp://127.0.0.1:5555")
        assert conn._on_connection_event is None

    def test_fire_connection_event_invokes_callback(self):
        """Given a Connection with callback,
        When _fire_connection_event is called,
        Then the callback receives the event dict.
        """
        from wheelhouse._core import Connection

        events = []
        conn = Connection(
            "tcp://127.0.0.1:5555",
            on_connection_event=lambda e: events.append(e),
        )
        conn._fire_connection_event("disconnected", reason="test")

        assert len(events) == 1
        assert events[0]["type"] == "disconnected"
        assert events[0]["reason"] == "test"

    def test_fire_connection_event_reconnecting(self):
        """Given a callback,
        When reconnecting event fires,
        Then it contains the attempt number.
        """
        from wheelhouse._core import Connection

        events = []
        conn = Connection(
            "tcp://127.0.0.1:5555",
            on_connection_event=lambda e: events.append(e),
        )
        conn._fire_connection_event("reconnecting", attempt=3)

        assert events[0]["type"] == "reconnecting"
        assert events[0]["attempt"] == 3

    def test_fire_connection_event_reconnect_failed(self):
        """Given a callback,
        When reconnect_failed event fires,
        Then it contains attempts count and error.
        """
        from wheelhouse._core import Connection

        events = []
        conn = Connection(
            "tcp://127.0.0.1:5555",
            on_connection_event=lambda e: events.append(e),
        )
        conn._fire_connection_event(
            "reconnect_failed", attempts=5, last_error="timeout"
        )

        assert events[0]["type"] == "reconnect_failed"
        assert events[0]["attempts"] == 5
        assert events[0]["last_error"] == "timeout"

    def test_fire_connection_event_no_callback_no_error(self):
        """Given a Connection without callback,
        When _fire_connection_event is called,
        Then no error is raised.
        """
        from wheelhouse._core import Connection

        conn = Connection("tcp://127.0.0.1:5555")
        # Should not raise
        conn._fire_connection_event("disconnected", reason="test")

    def test_callback_error_is_swallowed(self):
        """Given a callback that raises an exception,
        When _fire_connection_event is called,
        Then the exception is caught and logged (not propagated).
        """
        from wheelhouse._core import Connection

        def bad_callback(event):
            raise ValueError("callback error")

        conn = Connection(
            "tcp://127.0.0.1:5555",
            on_connection_event=bad_callback,
        )
        # Should not raise
        conn._fire_connection_event("disconnected", reason="test")
