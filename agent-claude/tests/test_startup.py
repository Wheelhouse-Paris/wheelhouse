"""Acceptance tests for Story 8.1: Container Skeleton, SDK Wiring, and Startup Sequence.

Tests verify all acceptance criteria for the agent-claude startup sequence.

Acceptance Criteria:
  AC1: Container starts with required env vars (WH_URL, WH_AGENT_NAME, WH_STREAMS, WH_PERSONA_PATH)
  AC2: Persona files loaded before wheelhouse.connect() with debug logging
  AC3: SDK connects to broker and logs info-level connection confirmation
  AC4: agent-claude imports sdk/python as local path dependency
  AC5: Missing ANTHROPIC_API_KEY -> exit code 1 with human-readable error
  AC6: Missing required env vars -> exit code 1 with human-readable error
  AC7: Missing persona files initialized as empty, container does NOT exit
"""

import os
import logging
from pathlib import Path
from unittest.mock import patch, MagicMock, AsyncMock

import pytest


# ---------------------------------------------------------------------------
# AC5: Missing ANTHROPIC_API_KEY -> exit code 1
# ---------------------------------------------------------------------------

class TestMissingApiKey:
    """ANTHROPIC_API_KEY absent at startup -> human-readable error + exit code 1."""

    def test_validate_env_missing_api_key(self):
        """Given ANTHROPIC_API_KEY is not set,
        When validate_env() is called,
        Then it raises AgentConfigError with a human-readable message.
        """
        from agent_claude.errors import AgentConfigError
        from agent_claude.main import validate_env

        env = {
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            with pytest.raises(AgentConfigError) as exc_info:
                validate_env()
            assert "ANTHROPIC_API_KEY" in str(exc_info.value)
            assert "wh secrets init" in str(exc_info.value)

    def test_missing_api_key_error_message_is_human_readable(self):
        """The error message must match the exact format from ADR-018."""
        from agent_claude.errors import AgentConfigError
        from agent_claude.main import validate_env

        env = {
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            with pytest.raises(AgentConfigError) as exc_info:
                validate_env()
            msg = str(exc_info.value)
            assert "ANTHROPIC_API_KEY is not set" in msg


# ---------------------------------------------------------------------------
# AC6: Missing required env vars -> exit code 1
# ---------------------------------------------------------------------------

class TestMissingRequiredEnvVars:
    """Each missing required env var produces human-readable error + exit code 1."""

    @pytest.mark.parametrize("missing_var", ["WH_URL", "WH_AGENT_NAME", "WH_STREAMS"])
    def test_validate_env_missing_required_var(self, missing_var):
        """Given a required env var is missing,
        When validate_env() is called,
        Then it raises AgentConfigError naming the missing variable.
        """
        from agent_claude.errors import AgentConfigError
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        del env[missing_var]
        with patch.dict(os.environ, env, clear=True):
            with pytest.raises(AgentConfigError) as exc_info:
                validate_env()
            assert missing_var in str(exc_info.value)

    def test_validate_env_all_present_succeeds(self):
        """Given all four required env vars are set,
        When validate_env() is called,
        Then it returns a config dict without raising.
        """
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main,events",
        }
        with patch.dict(os.environ, env, clear=True):
            config = validate_env()
            assert config["api_key"] == "sk-test-key"
            assert config["wh_url"] == "tcp://127.0.0.1:5555"
            assert config["agent_name"] == "donna"
            assert config["streams"] == ["main", "events"]

    def test_wh_persona_path_defaults_to_persona(self):
        """Given WH_PERSONA_PATH is not set,
        When validate_env() is called,
        Then persona_path defaults to /persona.
        """
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            config = validate_env()
            assert config["persona_path"] == "/persona"

    def test_wh_persona_path_override(self):
        """Given WH_PERSONA_PATH is set to a custom path,
        When validate_env() is called,
        Then persona_path uses the custom value.
        """
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
            "WH_PERSONA_PATH": "/custom/persona",
        }
        with patch.dict(os.environ, env, clear=True):
            config = validate_env()
            assert config["persona_path"] == "/custom/persona"

    def test_validate_env_empty_streams_after_split(self):
        """Given WH_STREAMS is set to only commas (no actual stream names),
        When validate_env() is called,
        Then it raises AgentConfigError.
        """
        from agent_claude.errors import AgentConfigError
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": ",,",
        }
        with patch.dict(os.environ, env, clear=True):
            with pytest.raises(AgentConfigError) as exc_info:
                validate_env()
            assert "WH_STREAMS" in str(exc_info.value)

    def test_claude_model_defaults(self):
        """Given CLAUDE_MODEL is not set,
        When validate_env() is called,
        Then model defaults to claude-3-5-sonnet-20241022.
        """
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            config = validate_env()
            assert config["model"] == "claude-3-5-sonnet-20241022"


# ---------------------------------------------------------------------------
# AC2: Persona files loaded with debug logging
# ---------------------------------------------------------------------------

class TestPersonaLoading:
    """Persona files loaded from WH_PERSONA_PATH before connect()."""

    def test_load_persona_existing_files(self, tmp_path):
        """Given SOUL.md, IDENTITY.md, MEMORY.md exist in persona_path,
        When load_persona() is called,
        Then all three files are loaded and their content is returned.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "SOUL.md").write_text("I am a helpful agent.")
        (tmp_path / "IDENTITY.md").write_text("My name is Donna.")
        (tmp_path / "MEMORY.md").write_text("Previous session notes.")

        persona = load_persona(str(tmp_path))
        assert persona.soul == "I am a helpful agent."
        assert persona.identity == "My name is Donna."
        assert persona.memory == "Previous session notes."

    def test_load_persona_logs_debug_per_file(self, tmp_path, caplog):
        """Given persona files exist,
        When load_persona() is called,
        Then a debug log entry per file includes the path and byte count.
        """
        from agent_claude.persona import load_persona

        content = "I am a helpful agent."
        (tmp_path / "SOUL.md").write_text(content)
        (tmp_path / "IDENTITY.md").write_text("identity")
        (tmp_path / "MEMORY.md").write_text("memory")

        with caplog.at_level(logging.DEBUG, logger="agent_claude"):
            load_persona(str(tmp_path))

        # Check debug logs mention file path and byte count
        debug_messages = [r.message for r in caplog.records if r.levelno == logging.DEBUG]
        assert any("SOUL.md" in m and str(len(content.encode())) in m for m in debug_messages)

    def test_load_persona_missing_soul_uses_empty(self, tmp_path, caplog):
        """Given SOUL.md is absent,
        When load_persona() is called,
        Then soul is empty string and a warning is logged.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "IDENTITY.md").write_text("identity")
        (tmp_path / "MEMORY.md").write_text("memory")

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            persona = load_persona(str(tmp_path))

        assert persona.soul == ""
        warn_messages = [r.message for r in caplog.records if r.levelno == logging.WARNING]
        assert any("SOUL.md" in m for m in warn_messages)

    def test_load_persona_missing_identity_uses_empty(self, tmp_path, caplog):
        """Given IDENTITY.md is absent,
        When load_persona() is called,
        Then identity is empty string and a warning is logged.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "SOUL.md").write_text("soul")
        (tmp_path / "MEMORY.md").write_text("memory")

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            persona = load_persona(str(tmp_path))

        assert persona.identity == ""
        warn_messages = [r.message for r in caplog.records if r.levelno == logging.WARNING]
        assert any("IDENTITY.md" in m for m in warn_messages)

    def test_load_persona_missing_memory_creates_empty_file(self, tmp_path, caplog):
        """Given MEMORY.md is absent,
        When load_persona() is called,
        Then an empty MEMORY.md file is created at the path (Story 2.5 consistency),
        And a warning is logged.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "SOUL.md").write_text("soul")
        (tmp_path / "IDENTITY.md").write_text("identity")

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            persona = load_persona(str(tmp_path))

        assert persona.memory == ""
        assert (tmp_path / "MEMORY.md").exists()
        warn_messages = [r.message for r in caplog.records if r.levelno == logging.WARNING]
        assert any("MEMORY.md" in m for m in warn_messages)

    def test_system_prompt_concatenation(self, tmp_path):
        """Given all persona files are loaded,
        When build_system_prompt() is called,
        Then the result is SOUL + '\\n\\n' + IDENTITY + '\\n\\n' + MEMORY.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "SOUL.md").write_text("soul content")
        (tmp_path / "IDENTITY.md").write_text("identity content")
        (tmp_path / "MEMORY.md").write_text("memory content")

        persona = load_persona(str(tmp_path))
        prompt = persona.build_system_prompt()
        assert prompt == "soul content\n\nidentity content\n\nmemory content"


# ---------------------------------------------------------------------------
# AC3: SDK connection and logging
# ---------------------------------------------------------------------------

class TestSdkConnection:
    """SDK connects to broker and logs info-level confirmation."""

    @pytest.mark.asyncio
    async def test_startup_connects_to_broker(self):
        """Given all env vars are set and persona loaded,
        When the startup sequence runs,
        Then wheelhouse.connect() is called with the WH_URL endpoint.
        """
        from agent_claude.main import run_startup

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
            "WH_PERSONA_PATH": "/tmp/test-persona",
        }

        with patch.dict(os.environ, env, clear=True):
            with patch("agent_claude.main.wheelhouse") as mock_wh:
                mock_conn = AsyncMock()
                mock_wh.connect = AsyncMock(return_value=mock_conn)
                with patch("agent_claude.main.load_persona") as mock_persona:
                    mock_persona.return_value = MagicMock(
                        soul="", identity="", memory="",
                        build_system_prompt=MagicMock(return_value="")
                    )
                    await run_startup()

                mock_wh.connect.assert_called_once_with("tcp://127.0.0.1:5555")

    @pytest.mark.asyncio
    async def test_startup_logs_connection_info(self, caplog):
        """Given the SDK connects successfully,
        When run_startup() completes,
        Then an info log confirms: 'agent-claude connected to broker at {URL} as {NAME}'.
        """
        from agent_claude.main import run_startup

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
            "WH_PERSONA_PATH": "/tmp/test-persona",
        }

        with patch.dict(os.environ, env, clear=True):
            with patch("agent_claude.main.wheelhouse") as mock_wh:
                mock_conn = AsyncMock()
                mock_wh.connect = AsyncMock(return_value=mock_conn)
                with patch("agent_claude.main.load_persona") as mock_persona:
                    mock_persona.return_value = MagicMock(
                        soul="", identity="", memory="",
                        build_system_prompt=MagicMock(return_value="")
                    )
                    with caplog.at_level(logging.INFO, logger="agent_claude"):
                        await run_startup()

        info_msgs = [r.message for r in caplog.records if r.levelno == logging.INFO]
        assert any(
            "connected to broker" in m and "tcp://127.0.0.1:5555" in m and "donna" in m
            for m in info_msgs
        )


# ---------------------------------------------------------------------------
# AC1: Startup sequence ordering
# ---------------------------------------------------------------------------

class TestStartupSequence:
    """Startup validates env, loads persona, then connects -- in order."""

    @pytest.mark.asyncio
    async def test_startup_calls_in_correct_order(self):
        """Given all prerequisites are met,
        When run_startup() executes,
        Then validate_env -> load_persona -> wheelhouse.connect is the call order.
        """
        from agent_claude.main import run_startup

        call_order = []

        env = {
            "ANTHROPIC_API_KEY": "sk-test-key",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
            "WH_PERSONA_PATH": "/tmp/test-persona",
        }

        with patch.dict(os.environ, env, clear=True):
            with patch("agent_claude.main.validate_env") as mock_validate:
                mock_validate.side_effect = lambda: (
                    call_order.append("validate_env"),
                    {
                        "api_key": "sk-test-key",
                        "wh_url": "tcp://127.0.0.1:5555",
                        "agent_name": "donna",
                        "streams": ["main"],
                        "persona_path": "/tmp/test-persona",
                        "context_path": "/tmp/test-context",
                        "model": "claude-3-5-sonnet-20241022",
                    },
                )[1]

                with patch("agent_claude.main.load_persona") as mock_persona:
                    def fake_load(path):
                        call_order.append("load_persona")
                        return MagicMock(
                            soul="", identity="", memory="",
                            build_system_prompt=MagicMock(return_value="")
                        )
                    mock_persona.side_effect = fake_load

                    with patch("agent_claude.main.wheelhouse") as mock_wh:
                        async def fake_connect(*args, **kwargs):
                            call_order.append("connect")
                            return AsyncMock()
                        mock_wh.connect = fake_connect

                        await run_startup()

        assert call_order == ["validate_env", "load_persona", "connect"]


# ---------------------------------------------------------------------------
# AC4: Package structure validation
# ---------------------------------------------------------------------------

class TestPackageStructure:
    """agent-claude package imports sdk/python as local path dependency."""

    def test_agent_claude_package_importable(self):
        """Given agent-claude is installed,
        When we import agent_claude,
        Then the import succeeds and __version__ is defined.
        """
        import agent_claude
        assert hasattr(agent_claude, "__version__")

    def test_agent_claude_has_git_sha(self):
        """Given agent-claude is installed,
        When we check __git_sha__,
        Then it is defined (may be 'unknown' in dev).
        """
        import agent_claude
        assert hasattr(agent_claude, "__git_sha__")
