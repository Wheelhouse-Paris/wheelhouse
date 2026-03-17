"""Acceptance tests for Story 10.3: Per-Stream Context Injection into Agent System Prompt.

Tests verify all acceptance criteria for loading CONTEXT.md files at startup
and injecting their content into the agent system prompt.

Acceptance Criteria:
  AC1: Context loaded at startup — system prompt includes content of CONTEXT.md for each subscribed stream
  AC2: Loaded once only — context is NOT re-read per message (restart required)
  AC3: Cross-reference — stream name labels allow matching with source_stream from messages
  AC4: Missing CONTEXT.md is non-fatal — warning logged, agent starts normally
"""

from __future__ import annotations

import logging
from pathlib import Path

import pytest


# ---------------------------------------------------------------------------
# AC1: Context loaded at startup
# ---------------------------------------------------------------------------

class TestContextLoadedAtStartup:
    """CONTEXT.md files for subscribed streams are loaded into the system prompt."""

    def test_load_stream_contexts_reads_existing_files(self, tmp_path: Path) -> None:
        """Given CONTEXT.md files exist for streams main, iktos, wh-admin,
        When load_stream_contexts() is called,
        Then it returns a dict mapping stream name -> content for each.
        """
        from agent_claude.context import load_stream_contexts

        # Create context files
        for name, content in [
            ("main", "Main stream context"),
            ("iktos", "Iktos client context"),
            ("wh-admin", "Admin channel context"),
        ]:
            ctx_dir = tmp_path / name
            ctx_dir.mkdir()
            (ctx_dir / "CONTEXT.md").write_text(content)

        result = load_stream_contexts(str(tmp_path), ["main", "iktos", "wh-admin"])

        assert result == {
            "main": "Main stream context",
            "iktos": "Iktos client context",
            "wh-admin": "Admin channel context",
        }

    def test_system_prompt_includes_stream_context_sections(self, tmp_path: Path) -> None:
        """Given persona files and stream contexts are loaded,
        When build_system_prompt() is called,
        Then the result includes labelled stream context sections after MEMORY.
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="I am Donna.",
            identity="An operator assistant.",
            memory="Previous notes.",
            stream_contexts={"main": "Main stream rules.", "iktos": "Iktos rules."},
        )
        prompt = persona.build_system_prompt()

        assert "## Stream Context: iktos" in prompt
        assert "Iktos rules." in prompt
        assert "## Stream Context: main" in prompt
        assert "Main stream rules." in prompt
        # Stream contexts come after memory
        assert prompt.index("Previous notes.") < prompt.index("## Stream Context:")

    def test_system_prompt_stream_contexts_alphabetical_order(self, tmp_path: Path) -> None:
        """Given multiple stream contexts,
        When build_system_prompt() is called,
        Then the stream context sections appear in alphabetical order.
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="soul",
            identity="identity",
            memory="memory",
            stream_contexts={"zebra": "Z content", "alpha": "A content", "main": "M content"},
        )
        prompt = persona.build_system_prompt()

        idx_alpha = prompt.index("## Stream Context: alpha")
        idx_main = prompt.index("## Stream Context: main")
        idx_zebra = prompt.index("## Stream Context: zebra")
        assert idx_alpha < idx_main < idx_zebra

    @pytest.mark.asyncio
    async def test_startup_loads_contexts_into_persona(self, tmp_path: Path) -> None:
        """Given WH_CONTEXT_PATH points to a directory with CONTEXT.md files,
        When run_startup() completes,
        Then the returned persona has stream_contexts populated.
        """
        import os
        from unittest.mock import patch, AsyncMock

        # Create context files
        ctx_dir = tmp_path / "context"
        ctx_dir.mkdir()
        (ctx_dir / "main").mkdir()
        (ctx_dir / "main" / "CONTEXT.md").write_text("Main context content")

        env = {
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
            "WH_PERSONA_PATH": str(tmp_path / "persona"),
            "WH_CONTEXT_PATH": str(ctx_dir),
        }

        # Create persona dir
        persona_dir = tmp_path / "persona"
        persona_dir.mkdir()
        (persona_dir / "SOUL.md").write_text("soul")
        (persona_dir / "IDENTITY.md").write_text("identity")
        (persona_dir / "MEMORY.md").write_text("memory")

        with patch.dict(os.environ, env, clear=True):
            with patch("agent_claude.main.wheelhouse") as mock_wh:
                mock_wh.connect = AsyncMock(return_value=AsyncMock())

                from agent_claude.main import run_startup
                result = await run_startup()

        assert result["persona"].stream_contexts == {"main": "Main context content"}


# ---------------------------------------------------------------------------
# AC2: Loaded once only
# ---------------------------------------------------------------------------

class TestLoadedOnceOnly:
    """Stream contexts are loaded at startup only, not re-read per message."""

    def test_stream_contexts_not_in_reload_memory(self, tmp_path: Path) -> None:
        """Given stream_contexts are set at startup,
        When reload_memory() is called,
        Then stream_contexts remain unchanged (only MEMORY.md is reloaded).
        """
        from agent_claude.persona import Persona

        persona_dir = tmp_path / "persona"
        persona_dir.mkdir()
        (persona_dir / "MEMORY.md").write_text("updated memory")

        persona = Persona(
            soul="soul",
            identity="identity",
            memory="old memory",
            stream_contexts={"main": "Main context loaded at startup"},
        )

        persona.reload_memory(str(persona_dir))

        # Memory should be updated
        assert persona.memory == "updated memory"
        # Stream contexts must NOT change
        assert persona.stream_contexts == {"main": "Main context loaded at startup"}

    def test_build_system_prompt_uses_startup_contexts_after_reload(self, tmp_path: Path) -> None:
        """Given stream contexts loaded at startup and MEMORY.md changed,
        When build_system_prompt() is called after reload_memory(),
        Then the prompt contains updated memory BUT original stream contexts.
        """
        from agent_claude.persona import Persona

        persona_dir = tmp_path / "persona"
        persona_dir.mkdir()
        (persona_dir / "MEMORY.md").write_text("new memory from git")

        persona = Persona(
            soul="soul",
            identity="identity",
            memory="original memory",
            stream_contexts={"main": "Main context from startup"},
        )

        persona.reload_memory(str(persona_dir))
        prompt = persona.build_system_prompt()

        assert "new memory from git" in prompt
        assert "Main context from startup" in prompt


# ---------------------------------------------------------------------------
# AC3: Cross-reference with source_stream
# ---------------------------------------------------------------------------

class TestCrossReferenceSourceStream:
    """Stream context sections are labelled so the agent can match source_stream."""

    def test_stream_context_section_header_matches_stream_name(self) -> None:
        """Given a stream named 'iktos' has context loaded,
        When build_system_prompt() is called,
        Then the section header is '## Stream Context: iktos',
        And the agent receiving source_stream='iktos' can cross-reference.
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="soul",
            identity="identity",
            memory="memory",
            stream_contexts={"iktos": "Iktos behavioral context"},
        )
        prompt = persona.build_system_prompt()

        # The header format must match exactly for cross-reference
        assert "## Stream Context: iktos" in prompt
        assert "Iktos behavioral context" in prompt


# ---------------------------------------------------------------------------
# AC4: Missing CONTEXT.md is non-fatal
# ---------------------------------------------------------------------------

class TestMissingContextNonFatal:
    """Missing or unreadable CONTEXT.md files are handled gracefully."""

    def test_missing_context_file_skipped_silently(self, tmp_path: Path) -> None:
        """Given a stream 'events' has no CONTEXT.md file,
        When load_stream_contexts() is called for ['main', 'events'],
        Then 'events' is not in the returned dict (no error).
        """
        from agent_claude.context import load_stream_contexts

        # Only create context for 'main'
        (tmp_path / "main").mkdir()
        (tmp_path / "main" / "CONTEXT.md").write_text("Main context")

        result = load_stream_contexts(str(tmp_path), ["main", "events"])

        assert "main" in result
        assert "events" not in result

    def test_missing_context_directory_skipped(self, tmp_path: Path) -> None:
        """Given the context directory for a stream doesn't exist at all,
        When load_stream_contexts() is called,
        Then that stream is skipped without error.
        """
        from agent_claude.context import load_stream_contexts

        result = load_stream_contexts(str(tmp_path), ["nonexistent"])

        assert result == {}

    def test_missing_context_logs_debug(self, tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
        """Given a CONTEXT.md file is missing for a stream,
        When load_stream_contexts() is called,
        Then a debug log is emitted.
        """
        from agent_claude.context import load_stream_contexts

        with caplog.at_level(logging.DEBUG, logger="agent_claude"):
            load_stream_contexts(str(tmp_path), ["missing-stream"])

        debug_msgs = [r.message for r in caplog.records if r.levelno == logging.DEBUG]
        assert any("missing-stream" in m for m in debug_msgs)

    def test_unreadable_context_logs_warning(self, tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
        """Given a CONTEXT.md file exists but is unreadable,
        When load_stream_contexts() is called,
        Then a warning is logged and the stream is skipped.
        """
        from agent_claude.context import load_stream_contexts

        # Create an unreadable file
        ctx_dir = tmp_path / "broken"
        ctx_dir.mkdir()
        ctx_file = ctx_dir / "CONTEXT.md"
        ctx_file.write_text("content")
        ctx_file.chmod(0o000)

        try:
            with caplog.at_level(logging.WARNING, logger="agent_claude"):
                result = load_stream_contexts(str(tmp_path), ["broken"])

            assert "broken" not in result
            warn_msgs = [r.message for r in caplog.records if r.levelno == logging.WARNING]
            assert any("broken" in m for m in warn_msgs)
        finally:
            # Restore permissions for cleanup
            ctx_file.chmod(0o644)

    def test_system_prompt_omits_streams_without_context(self) -> None:
        """Given some streams have context and some don't,
        When build_system_prompt() is called with only loaded contexts,
        Then only streams WITH context appear in the prompt.
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="soul",
            identity="identity",
            memory="memory",
            stream_contexts={"main": "Main rules"},
        )
        prompt = persona.build_system_prompt()

        assert "## Stream Context: main" in prompt
        assert "Main rules" in prompt
        # No "events" section (it was never loaded)
        assert "events" not in prompt

    def test_empty_context_path_returns_empty_dict(self, tmp_path: Path) -> None:
        """Given WH_CONTEXT_PATH points to an empty directory,
        When load_stream_contexts() is called,
        Then it returns an empty dict.
        """
        from agent_claude.context import load_stream_contexts

        result = load_stream_contexts(str(tmp_path), ["main", "events"])

        assert result == {}

    def test_no_streams_returns_empty_dict(self, tmp_path: Path) -> None:
        """Given an empty stream list,
        When load_stream_contexts() is called,
        Then it returns an empty dict.
        """
        from agent_claude.context import load_stream_contexts

        result = load_stream_contexts(str(tmp_path), [])

        assert result == {}


# ---------------------------------------------------------------------------
# Config: WH_CONTEXT_PATH env var
# ---------------------------------------------------------------------------

class TestContextPathConfig:
    """WH_CONTEXT_PATH env var handling in validate_env()."""

    def test_default_context_path(self) -> None:
        """Given WH_CONTEXT_PATH is not set,
        When validate_env() is called,
        Then context_path defaults to /context.
        """
        import os
        from unittest.mock import patch
        from agent_claude.main import validate_env

        env = {
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            config = validate_env()
            assert config["context_path"] == "/context"

    def test_custom_context_path(self) -> None:
        """Given WH_CONTEXT_PATH is set to a custom path,
        When validate_env() is called,
        Then context_path uses the custom value.
        """
        import os
        from unittest.mock import patch
        from agent_claude.main import validate_env

        env = {
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
            "WH_CONTEXT_PATH": "/custom/context",
        }
        with patch.dict(os.environ, env, clear=True):
            config = validate_env()
            assert config["context_path"] == "/custom/context"


# ---------------------------------------------------------------------------
# Persona construction with empty stream_contexts
# ---------------------------------------------------------------------------

class TestPersonaBackwardCompat:
    """Persona works correctly when no stream contexts are provided (backward compat)."""

    def test_persona_without_stream_contexts(self) -> None:
        """Given a Persona with no stream_contexts (default),
        When build_system_prompt() is called,
        Then the output is identical to pre-10.3 behavior.
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="soul content",
            identity="identity content",
            memory="memory content",
        )
        prompt = persona.build_system_prompt()
        assert prompt == "soul content\n\nidentity content\n\nmemory content"
