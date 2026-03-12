"""Acceptance tests for Story 6.2: SDK Progressive Disclosure — Three Working Examples.

These tests verify that the three SDK examples work correctly and that error
handling meets the acceptance criteria. All tests should FAIL before implementation
(TDD red phase) and pass after implementation.

Acceptance Criteria:
  AC1: Example 1 (type registration, no broker) runs without error, <=10 lines, prints schema
  AC2: Example 2 (publish/subscribe with core types) works end-to-end
  AC3: Example 3 (custom type + full surface loop) works in <50 lines
  AC4: Errors are catchable by type with .code matching ERRORS.md
"""

import os
import subprocess
import sys
from pathlib import Path

import pytest

# Project root (worktree root)
PROJECT_ROOT = Path(__file__).parent.parent.parent


# ---------------------------------------------------------------------------
# AC1: Example 1 — Type Registration Only (no broker)
# ---------------------------------------------------------------------------

class TestExample1TypeRegistration:
    """Example 1 must define a type, print its schema, and exit 0 with no broker."""

    def test_example_1_exists(self):
        """Example 1 file must exist at examples/01_register_type.py."""
        example_path = PROJECT_ROOT / "examples" / "01_register_type.py"
        assert example_path.exists(), f"Expected {example_path} to exist"

    def test_example_1_runs_without_error(self):
        """Example 1 runs standalone and exits with code 0."""
        example_path = PROJECT_ROOT / "examples" / "01_register_type.py"
        result = subprocess.run(
            [sys.executable, str(example_path)],
            capture_output=True,
            text=True,
            timeout=10,
            env={**os.environ, "WH_MOCK": "1"},
        )
        assert result.returncode == 0, f"Exit code {result.returncode}, stderr: {result.stderr}"

    def test_example_1_prints_type_info(self):
        """Example 1 output must include the registered type name."""
        example_path = PROJECT_ROOT / "examples" / "01_register_type.py"
        result = subprocess.run(
            [sys.executable, str(example_path)],
            capture_output=True,
            text=True,
            timeout=10,
            env={**os.environ, "WH_MOCK": "1"},
        )
        # Should print something about the registered type
        assert result.stdout.strip(), "Example 1 must produce output"
        # Should mention a type name (custom namespace, not wheelhouse.*)
        output_lower = result.stdout.lower()
        assert "type" in output_lower or "registered" in output_lower, (
            f"Output should mention type registration: {result.stdout}"
        )

    def test_example_1_is_10_lines_or_fewer(self):
        """Example 1 user-facing code must be <=10 lines (NFR-D1).

        Excludes: shebang, docstring, sys.path setup (infrastructure, not user code).
        """
        example_path = PROJECT_ROOT / "examples" / "01_register_type.py"
        content = example_path.read_text()
        # Count non-empty, non-comment, non-infrastructure lines (user-facing code)
        code_lines = [
            line for line in content.splitlines()
            if line.strip()
            and not line.strip().startswith("#")
            and not line.strip().startswith('"""')
            and "sys.path.insert" not in line
        ]
        assert len(code_lines) <= 10, (
            f"Example 1 has {len(code_lines)} code lines, must be <=10 (NFR-D1)"
        )

    def test_example_1_no_broker_connection(self):
        """Example 1 must not require a broker — no WH_URL or connect() call."""
        example_path = PROJECT_ROOT / "examples" / "01_register_type.py"
        content = example_path.read_text()
        # Should not call connect() with a real endpoint
        assert "connect()" not in content or "mock" in content.lower(), (
            "Example 1 should not require a real broker connection"
        )


# ---------------------------------------------------------------------------
# AC2: Example 2 — Publish/Subscribe with Core Types
# ---------------------------------------------------------------------------

class TestExample2PublishSubscribe:
    """Example 2 must connect, publish TextMessage, subscribe and receive it."""

    def test_example_2_exists(self):
        """Example 2 file must exist at examples/02_publish_subscribe.py."""
        example_path = PROJECT_ROOT / "examples" / "02_publish_subscribe.py"
        assert example_path.exists(), f"Expected {example_path} to exist"

    def test_example_2_runs_in_mock_mode(self):
        """Example 2 runs in mock mode and exits with code 0."""
        example_path = PROJECT_ROOT / "examples" / "02_publish_subscribe.py"
        result = subprocess.run(
            [sys.executable, str(example_path), "--mock"],
            capture_output=True,
            text=True,
            timeout=10,
            env={**os.environ, "WH_MOCK": "1"},
        )
        assert result.returncode == 0, f"Exit code {result.returncode}, stderr: {result.stderr}"

    def test_example_2_uses_text_message(self):
        """Example 2 must use TextMessage as the core type."""
        example_path = PROJECT_ROOT / "examples" / "02_publish_subscribe.py"
        content = example_path.read_text()
        assert "TextMessage" in content, "Example 2 must use TextMessage"

    def test_example_2_uses_context_manager(self):
        """Example 2 should demonstrate async with pattern for cleanup."""
        example_path = PROJECT_ROOT / "examples" / "02_publish_subscribe.py"
        content = example_path.read_text()
        assert "async with" in content, "Example 2 should use async context manager pattern"


# ---------------------------------------------------------------------------
# AC3: Example 3 — Custom Type + Full Surface Loop
# ---------------------------------------------------------------------------

class TestExample3CustomSurface:
    """Example 3 must register custom type, publish, subscribe, round-trip in <50 lines."""

    def test_example_3_exists(self):
        """Example 3 file must exist at examples/03_custom_surface.py."""
        example_path = PROJECT_ROOT / "examples" / "03_custom_surface.py"
        assert example_path.exists(), f"Expected {example_path} to exist"

    def test_example_3_runs_in_mock_mode(self):
        """Example 3 runs in mock mode and exits with code 0."""
        example_path = PROJECT_ROOT / "examples" / "03_custom_surface.py"
        result = subprocess.run(
            [sys.executable, str(example_path), "--mock"],
            capture_output=True,
            text=True,
            timeout=10,
            env={**os.environ, "WH_MOCK": "1"},
        )
        assert result.returncode == 0, f"Exit code {result.returncode}, stderr: {result.stderr}"

    def test_example_3_uses_register_type(self):
        """Example 3 must use @register_type decorator."""
        example_path = PROJECT_ROOT / "examples" / "03_custom_surface.py"
        content = example_path.read_text()
        assert "register_type" in content, "Example 3 must use @register_type"

    def test_example_3_under_50_lines(self):
        """Example 3 must be under 50 lines total."""
        example_path = PROJECT_ROOT / "examples" / "03_custom_surface.py"
        content = example_path.read_text()
        code_lines = [
            line for line in content.splitlines()
            if line.strip() and not line.strip().startswith("#")
        ]
        assert len(code_lines) < 50, (
            f"Example 3 has {len(code_lines)} code lines, must be <50"
        )

    def test_example_3_demonstrates_surface_class(self):
        """Example 3 should demonstrate the Surface base class."""
        example_path = PROJECT_ROOT / "examples" / "03_custom_surface.py"
        content = example_path.read_text()
        assert "Surface" in content, "Example 3 should demonstrate Surface class"


# ---------------------------------------------------------------------------
# AC4: Error Handling — Typed Exceptions with Codes
# ---------------------------------------------------------------------------

class TestErrorHandling:
    """Errors must be catchable by type with .code attribute matching ERRORS.md."""

    def test_errors_md_exists(self):
        """ERRORS.md must exist at project root."""
        errors_md = PROJECT_ROOT / "ERRORS.md"
        assert errors_md.exists(), f"Expected {errors_md} to exist"

    def test_publish_timeout_catchable_from_wheelhouse(self):
        """PublishTimeout must be importable as wheelhouse.PublishTimeout."""
        # This tests the re-export from __init__.py
        import wheelhouse
        assert hasattr(wheelhouse, "PublishTimeout"), (
            "wheelhouse.PublishTimeout must be available for `except wheelhouse.PublishTimeout`"
        )

    def test_connection_error_catchable_from_wheelhouse(self):
        """ConnectionError must be importable as wheelhouse.ConnectionError."""
        import wheelhouse
        assert hasattr(wheelhouse, "ConnectionError"), (
            "wheelhouse.ConnectionError must be available"
        )

    def test_error_has_code_attribute(self):
        """All Wheelhouse errors must have a .code attribute."""
        from wheelhouse.errors import PublishTimeout
        err = PublishTimeout("test error", code="PUBLISH_TIMEOUT")
        assert err.code == "PUBLISH_TIMEOUT"
        assert str(err) == "test error"

    def test_error_codes_match_errors_md(self):
        """All error codes in wheelhouse.errors must have entries in ERRORS.md."""
        from wheelhouse.errors import (
            ConnectionError as WhConnectionError,
            PublishTimeout,
            StreamNotFound,
            ReservedNamespaceError,
            InvalidTypeNameError,
            RegistryFullError,
        )

        errors_md = PROJECT_ROOT / "ERRORS.md"
        errors_content = errors_md.read_text()

        expected_codes = [
            "CONNECTION_ERROR",
            "PUBLISH_TIMEOUT",
            "STREAM_NOT_FOUND",
            "RESERVED_NAMESPACE",
            "INVALID_TYPE_NAME",
            "REGISTRY_FULL",
        ]

        for code in expected_codes:
            assert code in errors_content, (
                f"Error code {code} must be documented in ERRORS.md"
            )

    def test_not_connected_code_in_errors_md(self):
        """NOT_CONNECTED error code must be in ERRORS.md."""
        errors_md = PROJECT_ROOT / "ERRORS.md"
        errors_content = errors_md.read_text()
        assert "NOT_CONNECTED" in errors_content


# ---------------------------------------------------------------------------
# SDK Enhancement: connect(mock=True)
# ---------------------------------------------------------------------------

class TestConnectMockMode:
    """connect(mock=True) must return a MockConnection."""

    @pytest.mark.asyncio
    async def test_connect_mock_returns_mock_connection(self):
        """connect(mock=True) should return a MockConnection instance."""
        import wheelhouse
        from wheelhouse.testing import MockConnection
        conn = await wheelhouse.connect(mock=True)
        assert isinstance(conn, MockConnection), (
            f"Expected MockConnection, got {type(conn).__name__}"
        )

    @pytest.mark.asyncio
    async def test_mock_connection_echoes_to_subscribers(self):
        """MockConnection should echo published messages to subscribers (NFR-D4)."""
        import wheelhouse
        conn = await wheelhouse.connect(mock=True)

        received = []

        async def handler(msg):
            received.append(msg)

        await conn.subscribe("test-stream", handler)

        from wheelhouse.types import TextMessage
        msg = TextMessage(content="hello mock")
        await conn.publish("test-stream", msg)

        # After publish, subscriber should have received the message
        assert len(received) == 1, (
            f"Expected 1 received message, got {len(received)}"
        )
        await conn.close()
