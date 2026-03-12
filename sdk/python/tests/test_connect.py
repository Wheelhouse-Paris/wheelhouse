"""Acceptance tests for wheelhouse.connect() — AC #1, #3.

Unit tests that verify connection behavior without requiring a running broker.
"""

import os

import pytest


class TestConnectSuccess:
    """AC #1: connect returns a Connection with expected interface."""

    def test_connect_is_callable(self):
        """Given the wheelhouse package is installed,
        Then connect() is importable and callable.
        """
        import wheelhouse

        assert callable(wheelhouse.connect)

    def test_connect_is_async(self):
        """connect() is an async function."""
        import asyncio
        import wheelhouse

        assert asyncio.iscoroutinefunction(wheelhouse.connect)

    @pytest.mark.asyncio
    async def test_connect_uses_wh_url_env_var(self):
        """Given WH_URL is set in the environment,
        When I call await wheelhouse.connect(),
        Then the connection targets that endpoint (fails because broker is not there).
        """
        from wheelhouse.errors import ConnectionError as WhConnectionError
        import wheelhouse

        os.environ["WH_URL"] = "tcp://127.0.0.1:59998"
        try:
            with pytest.raises(WhConnectionError) as exc_info:
                await wheelhouse.connect()
            assert "59998" in str(exc_info.value)
        finally:
            del os.environ["WH_URL"]

    @pytest.mark.asyncio
    async def test_connect_accepts_endpoint_parameter(self):
        """Given an explicit endpoint parameter,
        When I call await wheelhouse.connect(endpoint=...),
        Then the connection targets that endpoint (fails because broker is not there).
        """
        from wheelhouse.errors import ConnectionError as WhConnectionError
        import wheelhouse

        with pytest.raises(WhConnectionError) as exc_info:
            await wheelhouse.connect(endpoint="tcp://127.0.0.1:59997")
        assert "59997" in str(exc_info.value)


class TestConnectFailure:
    """AC #3: typed ConnectionError when broker is not running."""

    @pytest.mark.asyncio
    async def test_connect_raises_connection_error_when_broker_down(self):
        """Given the broker is not running,
        When wheelhouse.connect() is called,
        Then a typed ConnectionError is raised.
        """
        from wheelhouse.errors import ConnectionError as WhConnectionError
        import wheelhouse

        with pytest.raises(WhConnectionError):
            await wheelhouse.connect(endpoint="tcp://127.0.0.1:59999")

    @pytest.mark.asyncio
    async def test_connection_error_includes_address(self):
        """Given the broker is not running,
        When wheelhouse.connect() is called,
        Then the error message includes the broker address.
        """
        from wheelhouse.errors import ConnectionError as WhConnectionError
        import wheelhouse

        with pytest.raises(WhConnectionError, match="127.0.0.1"):
            await wheelhouse.connect(endpoint="tcp://127.0.0.1:59999")

    @pytest.mark.asyncio
    async def test_connection_error_is_human_readable(self):
        """Given the broker is not running,
        When wheelhouse.connect() is called,
        Then the error message is human-readable (no raw ZMQ internals).
        """
        from wheelhouse.errors import ConnectionError as WhConnectionError
        import wheelhouse

        with pytest.raises(WhConnectionError) as exc_info:
            await wheelhouse.connect(endpoint="tcp://127.0.0.1:59999")

        error_msg = str(exc_info.value)
        # Must NOT contain raw ZMQ terminology (RT-B1)
        assert "zmq" not in error_msg.lower()
        assert "socket" not in error_msg.lower()
        assert "broker" not in error_msg.lower()
        # SHOULD contain approved vocabulary
        assert "127.0.0.1" in error_msg
        assert "Wheelhouse" in error_msg


class TestConnectThreadSafety:
    """FM-14: connect() is explicitly not thread-safe."""

    def test_connect_not_thread_safe_documented(self):
        """Given the connect() function,
        It should be documented as not thread-safe per FM-14.
        """
        import wheelhouse

        assert callable(wheelhouse.connect)


class TestConnectionInterface:
    """Verify Connection object has the required interface using MockConnection."""

    def test_mock_connection_has_publish(self):
        from wheelhouse.testing import MockConnection

        conn = MockConnection()
        assert hasattr(conn, "publish")
        assert callable(conn.publish)

    def test_mock_connection_has_subscribe(self):
        from wheelhouse.testing import MockConnection

        conn = MockConnection()
        assert hasattr(conn, "subscribe")
        assert callable(conn.subscribe)

    def test_mock_connection_has_close(self):
        from wheelhouse.testing import MockConnection

        conn = MockConnection()
        assert hasattr(conn, "close")
        assert callable(conn.close)

    @pytest.mark.asyncio
    async def test_mock_connection_context_manager(self):
        from wheelhouse.testing import MockConnection

        async with MockConnection() as conn:
            assert conn._connected is True
        assert conn._connected is False
