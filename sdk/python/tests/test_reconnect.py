"""Acceptance tests for auto-reconnect — AC #4.

Unit tests verifying reconnect backoff logic without a running broker.
"""

import pytest


class TestAutoReconnect:
    """AC #4: automatic reconnect with exponential backoff."""

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
        assert hasattr(conn, "_registered_types")
        assert isinstance(conn._registered_types, dict)

    def test_register_type(self):
        """Given a connection,
        When register_type() is called,
        Then the type is tracked for reconnect re-registration.
        """
        from wheelhouse._core import Connection
        from wheelhouse.types import TextMessage

        conn = Connection("tcp://127.0.0.1:5555")
        conn.register_type("TextMessage", TextMessage)
        assert "TextMessage" in conn._registered_types
        assert conn._registered_types["TextMessage"] is TextMessage
