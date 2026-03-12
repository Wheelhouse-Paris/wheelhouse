"""Pytest fixtures for testing Wheelhouse surfaces and handlers in mock mode.

Usage in your conftest.py:
    from wheelhouse.fixtures import mock_connection, mock_surface

Or copy individual fixtures into your own conftest.py.

These fixtures provide MockConnection instances that work without a running
Wheelhouse instance or Podman installation (NFR-D4).
"""

from __future__ import annotations

from typing import AsyncGenerator

import pytest
import pytest_asyncio

from wheelhouse.testing import MockConnection


@pytest_asyncio.fixture
async def mock_connection() -> AsyncGenerator[MockConnection, None]:
    """Provide a MockConnection for testing publish/subscribe without Wheelhouse.

    The connection is automatically closed when the test completes.

    Example:
        async def test_my_surface(mock_connection):
            received = []
            await mock_connection.subscribe("stream", lambda m: received.append(m))
            await mock_connection.publish("stream", TextMessage(content="hi"))
            assert len(received) == 1
    """
    conn = MockConnection()
    try:
        yield conn
    finally:
        await conn.close()


@pytest_asyncio.fixture
async def mock_surface() -> AsyncGenerator[MockConnection, None]:
    """Provide a MockConnection suitable for wrapping in a Surface subclass.

    Identical to mock_connection — use this when your test creates a Surface
    that needs a connection to delegate to.

    Example:
        async def test_custom_surface(mock_surface):
            surface = MySurface(mock_surface)
            await surface.publish("stream", TextMessage(content="hello"))
            assert len(mock_surface.published) == 1
    """
    conn = MockConnection()
    try:
        yield conn
    finally:
        await conn.close()
