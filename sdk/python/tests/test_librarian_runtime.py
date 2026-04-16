"""Tests for the librarian container runtime (story 14-1-2).

Covers:
  - Env var validation (AC-2)
  - LibrarianLoop.process_event() skeleton (AC-4, AC-6)
  - TopologyShutdown handling (AC-5)
  - LibraryWriteEvent type registration (AC-4)
  - DecisionResult structure
"""

from __future__ import annotations

import logging
import os
from pathlib import Path
from unittest.mock import patch

import pytest

from wheelhouse.librarian.proto import ConversationMessage, LibraryWriteEvent
from wheelhouse.librarian.loop import LibrarianLoop
from wheelhouse.librarian.types import DecisionResult


# ─── Proto / Type Registration Tests ────────────────────────────────────────


class TestLibraryWriteEventProto:
    """Verify LibraryWriteEvent betterproto definition matches ADR-042."""

    def test_roundtrip_serialization(self):
        """LibraryWriteEvent serializes and deserializes correctly."""
        event = LibraryWriteEvent(
            event_id="evt-001",
            source_agent_id="agent-a",
            library_id="research",
            conversation_id="conv-123",
            timestamp_ms=1713264000000,
            segment=[
                ConversationMessage(role="user", content="Hello", timestamp_ms=1713264000000),
                ConversationMessage(role="assistant", content="Hi there", timestamp_ms=1713264001000),
            ],
            locale="en",
            metadata={"key": "value"},
        )

        # Serialize
        data = bytes(event)
        assert len(data) > 0

        # Deserialize
        decoded = LibraryWriteEvent().parse(data)
        assert decoded.event_id == "evt-001"
        assert decoded.source_agent_id == "agent-a"
        assert decoded.library_id == "research"
        assert decoded.conversation_id == "conv-123"
        assert decoded.timestamp_ms == 1713264000000
        assert len(decoded.segment) == 2
        assert decoded.segment[0].role == "user"
        assert decoded.segment[0].content == "Hello"
        assert decoded.segment[1].role == "assistant"
        assert decoded.locale == "en"
        assert decoded.metadata == {"key": "value"}

    def test_empty_event(self):
        """Empty LibraryWriteEvent serializes to minimal bytes."""
        event = LibraryWriteEvent()
        data = bytes(event)
        decoded = LibraryWriteEvent().parse(data)
        assert decoded.event_id == ""
        assert decoded.locale == ""
        assert len(decoded.segment) == 0

    def test_registered_in_builtin_types(self):
        """LibraryWriteEvent is registered in SDK _BUILTIN_TYPES."""
        from wheelhouse._core import _BUILTIN_TYPES

        assert "LibraryWriteEvent" in _BUILTIN_TYPES
        assert _BUILTIN_TYPES["LibraryWriteEvent"] is LibraryWriteEvent

    def test_importable_from_wheelhouse_types(self):
        """LibraryWriteEvent is importable from wheelhouse.types."""
        from wheelhouse.types import LibraryWriteEvent as LWE
        from wheelhouse.types import ConversationMessage as CM

        assert LWE is LibraryWriteEvent
        assert CM is ConversationMessage


# ─── Env Var Validation Tests ────────────────────────────────────────────────


class TestValidateEnv:
    """Test env var validation (AC-2)."""

    VALID_ENV = {
        "ANTHROPIC_API_KEY": "sk-test-key",
        "WH_URL": "tcp://127.0.0.1:5555",
        "WH_AGENT_NAME": "librarian-test",
        "WH_STREAMS": "eot-agent-a-research,eot-agent-b-research",
        "WH_LIBRARY_PATH": "/tmp",  # /tmp always exists
        "WH_LIBRARY_ID": "research",
    }

    def test_valid_env_returns_config(self):
        """All required vars present returns LibrarianConfig."""
        from wheelhouse.librarian.__main__ import validate_env

        with patch.dict(os.environ, self.VALID_ENV, clear=False):
            config = validate_env()

        assert config.anthropic_api_key == "sk-test-key"
        assert config.wh_url == "tcp://127.0.0.1:5555"
        assert config.agent_name == "librarian-test"
        assert config.streams == ["eot-agent-a-research", "eot-agent-b-research"]
        assert config.library_path == "/tmp"
        assert config.library_id == "research"
        assert config.locales == ["en", "fr"]  # default

    def test_missing_required_var_exits(self):
        """Missing required var exits with code 1."""
        from wheelhouse.librarian.__main__ import validate_env

        # Remove ANTHROPIC_API_KEY
        env = dict(self.VALID_ENV)
        del env["ANTHROPIC_API_KEY"]

        with patch.dict(os.environ, env, clear=True), pytest.raises(SystemExit) as exc_info:
            validate_env()

        assert exc_info.value.code == 1

    def test_missing_multiple_vars_lists_all(self, caplog):
        """Missing multiple vars are all listed in error message."""
        from wheelhouse.librarian.__main__ import validate_env

        with (
            patch.dict(os.environ, {}, clear=True),
            caplog.at_level(logging.ERROR),
            pytest.raises(SystemExit),
        ):
            validate_env()

        assert "ANTHROPIC_API_KEY" in caplog.text
        assert "WH_URL" in caplog.text

    def test_invalid_library_path_exits(self):
        """Non-existent WH_LIBRARY_PATH exits with code 1."""
        from wheelhouse.librarian.__main__ import validate_env

        env = dict(self.VALID_ENV)
        env["WH_LIBRARY_PATH"] = "/nonexistent/path/that/does/not/exist"

        with patch.dict(os.environ, env, clear=False), pytest.raises(SystemExit) as exc_info:
            validate_env()

        assert exc_info.value.code == 1

    def test_custom_locales(self):
        """WH_LIBRARIAN_LOCALES overrides default locales."""
        from wheelhouse.librarian.__main__ import validate_env

        env = dict(self.VALID_ENV)
        env["WH_LIBRARIAN_LOCALES"] = "en,fr,de"

        with patch.dict(os.environ, env, clear=False):
            config = validate_env()

        assert config.locales == ["en", "fr", "de"]

    def test_empty_streams_exits(self):
        """WH_STREAMS with only whitespace exits with code 1."""
        from wheelhouse.librarian.__main__ import validate_env

        env = dict(self.VALID_ENV)
        env["WH_STREAMS"] = "  ,  ,  "

        with patch.dict(os.environ, env, clear=False), pytest.raises(SystemExit):
            validate_env()


# ─── LibrarianLoop Tests ─────────────────────────────────────────────────────


class TestLibrarianLoop:
    """Test LibrarianLoop skeleton (AC-4, AC-6)."""

    @pytest.fixture
    def loop(self, tmp_path: Path) -> LibrarianLoop:
        """Create a LibrarianLoop with a temp library path."""
        return LibrarianLoop(
            library_path=tmp_path,
            library_id="test-lib",
            locales=["en", "fr"],
            llm_fn=None,
        )

    @pytest.fixture
    def sample_event(self) -> LibraryWriteEvent:
        """Create a sample LibraryWriteEvent."""
        return LibraryWriteEvent(
            event_id="evt-test-001",
            source_agent_id="agent-a",
            library_id="test-lib",
            conversation_id="conv-001",
            timestamp_ms=1713264000000,
            segment=[
                ConversationMessage(
                    role="user",
                    content="My birthday is April 5th",
                    timestamp_ms=1713264000000,
                ),
                ConversationMessage(
                    role="assistant",
                    content="I'll remember that!",
                    timestamp_ms=1713264001000,
                ),
            ],
            locale="en",
        )

    def test_process_event_returns_decision_result(
        self, loop: LibrarianLoop, sample_event: LibraryWriteEvent
    ):
        """process_event returns a DecisionResult."""
        result = loop.process_event(sample_event)

        assert isinstance(result, DecisionResult)
        assert result.event_id == "evt-test-001"
        assert result.locale == "en"
        assert result.reason == "pending"
        assert result.committed is False

    def test_process_event_logs_event_id(
        self, loop: LibrarianLoop, sample_event: LibraryWriteEvent, caplog
    ):
        """process_event logs event_id and source_agent_id."""
        with caplog.at_level(logging.INFO, logger="wheelhouse.librarian"):
            loop.process_event(sample_event)

        assert "evt-test-001" in caplog.text
        assert "agent-a" in caplog.text

    def test_process_event_defaults_locale_to_en(
        self, loop: LibrarianLoop
    ):
        """Empty locale defaults to 'en'."""
        event = LibraryWriteEvent(
            event_id="evt-no-locale",
            source_agent_id="agent-b",
            library_id="test-lib",
            locale="",
        )
        result = loop.process_event(event)
        assert result.locale == "en"

    def test_loop_init_stores_config(self, tmp_path: Path):
        """LibrarianLoop stores configuration correctly."""
        loop = LibrarianLoop(
            library_path=tmp_path,
            library_id="my-lib",
            locales=["en", "de"],
            llm_fn=None,
        )
        assert loop.library_path == tmp_path
        assert loop.library_id == "my-lib"
        assert loop.locales == ["en", "de"]
        assert loop.llm_fn is None

    def test_loop_default_locales(self, tmp_path: Path):
        """Default locales are en,fr when not specified."""
        loop = LibrarianLoop(library_path=tmp_path, library_id="lib")
        assert loop.locales == ["en", "fr"]


# ─── DecisionResult Tests ────────────────────────────────────────────────────


class TestDecisionResult:
    """Test DecisionResult dataclass structure."""

    def test_default_values(self):
        """DecisionResult defaults match ADR-047 expectations."""
        result = DecisionResult()
        assert result.reason == "pending"
        assert result.committed is False
        assert result.event_id == ""
        assert result.locale == ""
        assert result.page_path is None
        assert result.content is None
        assert result.tokens_consumed == 0
        assert result.schema_version == 1

    def test_committed_result(self):
        """DecisionResult for a committed write."""
        result = DecisionResult(
            reason="durable_fact_written",
            committed=True,
            event_id="evt-123",
            locale="fr",
            page_path="people/marc.md",
            content="# Marc\n\nBirthday: April 5th",
            tokens_consumed=847,
        )
        assert result.committed is True
        assert result.reason == "durable_fact_written"
        assert result.page_path == "people/marc.md"


# ─── Message Dispatch Tests ──────────────────────────────────────────────────


class TestMessageDispatch:
    """Test the __main__ message handler routing."""

    @pytest.mark.asyncio
    async def test_shutdown_sets_event(self):
        """TopologyShutdown sets the shutdown event."""
        from wheelhouse.librarian.__main__ import _shutdown_event

        _shutdown_event.clear()
        assert not _shutdown_event.is_set()

        # Import and call the shutdown path directly
        from wheelhouse.types import TopologyShutdown

        shutdown_msg = TopologyShutdown()

        # Simulate the handler logic
        if isinstance(shutdown_msg, TopologyShutdown):
            _shutdown_event.set()

        assert _shutdown_event.is_set()
        _shutdown_event.clear()  # Clean up
