"""Shared pytest fixtures for Wheelhouse SDK tests."""

from __future__ import annotations

import pytest

from wheelhouse.testing import MockConnection


@pytest.fixture
def mock_connection() -> MockConnection:
    """Provide a MockConnection for testing without ZMQ."""
    return MockConnection()
