"""Acceptance tests for Story 12.4: Layered Context Injection in agent-claude.

Tests verify all acceptance criteria for assembling startup context in 5 layers
(ADR-033, E12-10, E12-11, E12-12).

Acceptance Criteria:
  AC-1: All 5 layers assembled in correct order (L0→L1→L2→L3→L4)
  AC-2: Missing L0 (no capabilities.json) → warning, agent starts
  AC-3: Missing L1 (no wh binary) → warning, agent starts
  AC-4: Missing L2 (topology plan fails) → warning, agent starts
  AC-5: Total context size logged at startup
  AC-6: Backward compat — agent works with no L0/L1/L2 sources
"""

from __future__ import annotations

import json
import logging
import os
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest


# ---------------------------------------------------------------------------
# AC-1: All 5 layers assembled in correct order
# ---------------------------------------------------------------------------

class TestLayerOrderAC1:
    """All 5 layers assemble in fixed order L0→L1→L2→L3→L4 (E12-10)."""

    def test_all_layers_in_correct_order(self, tmp_path: Path) -> None:
        """Given all layer sources are available,
        When the system prompt is built,
        Then L0 content appears before L1, L1 before L2, L2 before L3, L3 before L4.
        """
        from agent_claude.persona import Persona

        platform_context = (
            "## Wheelhouse Capabilities\n\ncapabilities content\n\n"
            "## CLI Reference\n\ncli reference content\n\n"
            "## Topology State\n\ntopology state content"
        )

        persona = Persona(
            soul="soul content",
            identity="identity content",
            memory="memory content",
            stream_contexts={"main": "stream context content"},
            platform_context=platform_context,
        )
        prompt = persona.build_system_prompt()

        # Verify order: L0 < L1 < L2 < L3 < L4
        idx_l0 = prompt.index("## Wheelhouse Capabilities")
        idx_l1 = prompt.index("## CLI Reference")
        idx_l2 = prompt.index("## Topology State")
        idx_l3 = prompt.index("soul content")
        idx_l4 = prompt.index("## Stream Context: main")

        assert idx_l0 < idx_l1 < idx_l2 < idx_l3 < idx_l4

    def test_platform_context_before_persona(self) -> None:
        """Given platform context is set,
        When build_system_prompt() is called,
        Then platform context appears before persona content.
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="SOUL_HERE",
            identity="IDENTITY_HERE",
            memory="MEMORY_HERE",
            platform_context="PLATFORM_CONTEXT_HERE",
        )
        prompt = persona.build_system_prompt()

        assert prompt.index("PLATFORM_CONTEXT_HERE") < prompt.index("SOUL_HERE")


# ---------------------------------------------------------------------------
# AC-2: Missing L0 (no capabilities.json) → warning, agent starts
# ---------------------------------------------------------------------------

class TestMissingL0AC2:
    """Missing capabilities.json is skipped with warning."""

    def test_missing_capabilities_returns_none(self, tmp_path: Path) -> None:
        """Given /etc/wh/capabilities.json does not exist,
        When load_l0_capabilities() is called,
        Then it returns None.
        """
        from agent_claude.layers import load_l0_capabilities

        result = load_l0_capabilities(str(tmp_path / "nonexistent.json"))
        assert result is None

    def test_missing_capabilities_logs_warning(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture
    ) -> None:
        """Given capabilities.json does not exist,
        When load_l0_capabilities() is called,
        Then a warning mentioning 'capabilities.json' is logged.
        """
        from agent_claude.layers import load_l0_capabilities

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            load_l0_capabilities(str(tmp_path / "capabilities.json"))

        warn_msgs = [r.message for r in caplog.records if r.levelno == logging.WARNING]
        assert any("capabilities.json" in m for m in warn_msgs)

    def test_valid_capabilities_returns_section(self, tmp_path: Path) -> None:
        """Given capabilities.json exists with valid JSON,
        When load_l0_capabilities() is called,
        Then it returns a formatted section with the content.
        """
        from agent_claude.layers import load_l0_capabilities

        caps = {"features": [{"name": "streams", "status": "available"}]}
        caps_file = tmp_path / "capabilities.json"
        caps_file.write_text(json.dumps(caps))

        result = load_l0_capabilities(str(caps_file))
        assert result is not None
        assert "## Wheelhouse Capabilities" in result
        assert '"streams"' in result

    def test_invalid_json_capabilities_returns_none(self, tmp_path: Path) -> None:
        """Given capabilities.json exists but contains invalid JSON,
        When load_l0_capabilities() is called,
        Then it returns None and logs a warning.
        """
        from agent_claude.layers import load_l0_capabilities

        caps_file = tmp_path / "capabilities.json"
        caps_file.write_text("not valid json {{{")

        result = load_l0_capabilities(str(caps_file))
        assert result is None


# ---------------------------------------------------------------------------
# AC-3: Missing L1 (wh binary not found) → warning, agent starts
# ---------------------------------------------------------------------------

class TestMissingL1AC3:
    """Missing wh binary is skipped with warning."""

    def test_no_wh_binary_no_file_returns_none(self, tmp_path: Path) -> None:
        """Given wh binary is not on PATH and no cli-reference.md file exists,
        When load_l1_cli_reference() is called,
        Then it returns None.
        """
        from agent_claude.layers import load_l1_cli_reference

        with patch("agent_claude.layers.subprocess.run", side_effect=FileNotFoundError):
            result = load_l1_cli_reference(
                file_path=str(tmp_path / "cli-reference.md")
            )
        assert result is None

    def test_no_wh_binary_logs_warning(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture
    ) -> None:
        """Given wh binary is not on PATH,
        When load_l1_cli_reference() is called,
        Then a warning mentioning 'wh' is logged.
        """
        from agent_claude.layers import load_l1_cli_reference

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            with patch(
                "agent_claude.layers.subprocess.run",
                side_effect=FileNotFoundError,
            ):
                load_l1_cli_reference(
                    file_path=str(tmp_path / "cli-reference.md")
                )

        warn_msgs = [r.message for r in caplog.records if r.levelno == logging.WARNING]
        assert any("wh" in m.lower() for m in warn_msgs)

    def test_cli_reference_file_exists(self, tmp_path: Path) -> None:
        """Given /etc/wh/cli-reference.md exists,
        When load_l1_cli_reference() is called,
        Then it returns the file content as a formatted section.
        """
        from agent_claude.layers import load_l1_cli_reference

        ref_file = tmp_path / "cli-reference.md"
        ref_file.write_text("# wh CLI Reference\n\nUsage: wh <command>")

        result = load_l1_cli_reference(file_path=str(ref_file))
        assert result is not None
        assert "## CLI Reference" in result
        assert "wh <command>" in result

    def test_wh_reference_subprocess_success(self) -> None:
        """Given wh binary exists and `wh reference` succeeds,
        When load_l1_cli_reference() is called with no file,
        Then it returns the subprocess output as a formatted section.
        """
        from agent_claude.layers import load_l1_cli_reference

        mock_proc = MagicMock()
        mock_proc.returncode = 0
        mock_proc.stdout = "wh reference output"

        with patch("agent_claude.layers.subprocess.run", return_value=mock_proc):
            result = load_l1_cli_reference(file_path="/nonexistent/path")

        assert result is not None
        assert "## CLI Reference" in result
        assert "wh reference output" in result

    def test_wh_reference_subprocess_timeout(
        self, caplog: pytest.LogCaptureFixture
    ) -> None:
        """Given `wh reference` times out,
        When load_l1_cli_reference() is called,
        Then it returns None and logs a warning.
        """
        import subprocess as sp
        from agent_claude.layers import load_l1_cli_reference

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            with patch(
                "agent_claude.layers.subprocess.run",
                side_effect=sp.TimeoutExpired(cmd="wh", timeout=5),
            ):
                result = load_l1_cli_reference(file_path="/nonexistent/path")

        assert result is None
        warn_msgs = [r.message for r in caplog.records if r.levelno == logging.WARNING]
        assert any("timed out" in m for m in warn_msgs)


# ---------------------------------------------------------------------------
# AC-4: Missing L2 (topology plan fails) → warning, agent starts
# ---------------------------------------------------------------------------

class TestMissingL2AC4:
    """Failed topology plan is skipped with warning."""

    def test_no_wh_binary_returns_none(self) -> None:
        """Given wh binary is not on PATH,
        When load_l2_topology_state() is called,
        Then it returns None.
        """
        from agent_claude.layers import load_l2_topology_state

        with patch(
            "agent_claude.layers.subprocess.run",
            side_effect=FileNotFoundError,
        ):
            result = load_l2_topology_state()
        assert result is None

    def test_topology_plan_fails_returns_none(self) -> None:
        """Given `wh topology plan --format json` returns non-zero exit code,
        When load_l2_topology_state() is called,
        Then it returns None.
        """
        from agent_claude.layers import load_l2_topology_state

        mock_proc = MagicMock()
        mock_proc.returncode = 1
        mock_proc.stderr = "error: no topology found"

        with patch("agent_claude.layers.subprocess.run", return_value=mock_proc):
            result = load_l2_topology_state()
        assert result is None

    def test_topology_plan_fails_logs_warning(
        self, caplog: pytest.LogCaptureFixture
    ) -> None:
        """Given topology plan fails,
        When load_l2_topology_state() is called,
        Then a warning mentioning 'topology plan' is logged.
        """
        from agent_claude.layers import load_l2_topology_state

        mock_proc = MagicMock()
        mock_proc.returncode = 1
        mock_proc.stderr = "error"

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            with patch("agent_claude.layers.subprocess.run", return_value=mock_proc):
                load_l2_topology_state()

        warn_msgs = [r.message for r in caplog.records if r.levelno == logging.WARNING]
        assert any("topology plan" in m for m in warn_msgs)

    def test_topology_plan_success(self) -> None:
        """Given `wh topology plan --format json` succeeds,
        When load_l2_topology_state() is called,
        Then it returns a formatted section with the JSON output.
        """
        from agent_claude.layers import load_l2_topology_state

        topology = {"agents": [{"name": "donna"}], "streams": [{"name": "main"}]}
        mock_proc = MagicMock()
        mock_proc.returncode = 0
        mock_proc.stdout = json.dumps(topology)

        with patch("agent_claude.layers.subprocess.run", return_value=mock_proc):
            result = load_l2_topology_state()

        assert result is not None
        assert "## Topology State" in result
        assert '"donna"' in result

    def test_topology_plan_invalid_json_returns_none(self) -> None:
        """Given topology plan returns non-JSON output,
        When load_l2_topology_state() is called,
        Then it returns None.
        """
        from agent_claude.layers import load_l2_topology_state

        mock_proc = MagicMock()
        mock_proc.returncode = 0
        mock_proc.stdout = "not json output"

        with patch("agent_claude.layers.subprocess.run", return_value=mock_proc):
            result = load_l2_topology_state()
        assert result is None


# ---------------------------------------------------------------------------
# AC-5: Total context size logged at startup (E12-12)
# ---------------------------------------------------------------------------

class TestTotalContextSizeAC5:
    """Total context character count is logged at INFO level."""

    @pytest.mark.asyncio
    async def test_total_context_size_logged(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture
    ) -> None:
        """Given the agent completes layer assembly,
        When run_startup() is called,
        Then the total character count is logged at INFO level.
        """
        from unittest.mock import AsyncMock

        from agent_claude.main import run_startup

        # Create persona files
        persona_dir = tmp_path / "persona"
        persona_dir.mkdir()
        (persona_dir / "SOUL.md").write_text("soul")
        (persona_dir / "IDENTITY.md").write_text("identity")
        (persona_dir / "MEMORY.md").write_text("memory")

        env = {
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
            "WH_PERSONA_PATH": str(persona_dir),
            "WH_CONTEXT_PATH": str(tmp_path / "context"),
        }

        with patch.dict(os.environ, env, clear=True):
            with patch("agent_claude.main.wheelhouse") as mock_wh:
                mock_wh.connect = AsyncMock(return_value=AsyncMock())
                with patch("agent_claude.main.assemble_platform_context", return_value=""):
                    with caplog.at_level(logging.INFO, logger="agent_claude"):
                        await run_startup()

        info_msgs = [r.message for r in caplog.records if r.levelno == logging.INFO]
        assert any(
            "total characters" in m and "L0-L4" in m
            for m in info_msgs
        ), f"Expected total context size log, got: {info_msgs}"


# ---------------------------------------------------------------------------
# AC-6: Backward compat — no L0/L1/L2 sources
# ---------------------------------------------------------------------------

class TestBackwardCompatAC6:
    """Agent works when no L0/L1/L2 sources are available."""

    def test_assemble_platform_context_all_missing(
        self, caplog: pytest.LogCaptureFixture
    ) -> None:
        """Given no capabilities.json, no wh binary, no topology plan,
        When assemble_platform_context() is called,
        Then it returns an empty string and three warnings are logged.
        """
        from agent_claude.layers import assemble_platform_context

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            with patch(
                "agent_claude.layers.load_l0_capabilities", return_value=None
            ):
                with patch(
                    "agent_claude.layers.load_l1_cli_reference", return_value=None
                ):
                    with patch(
                        "agent_claude.layers.load_l2_topology_state",
                        return_value=None,
                    ):
                        result = assemble_platform_context()

        assert result == ""

    def test_persona_without_platform_context_backward_compat(self) -> None:
        """Given no platform context is set (default empty string),
        When build_system_prompt() is called,
        Then the output matches pre-12.4 behavior (L3+L4 only).
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="soul content",
            identity="identity content",
            memory="memory content",
        )
        prompt = persona.build_system_prompt()
        assert prompt.startswith("soul content\n\nidentity content\n\nmemory content")
        assert "## Output Format" in prompt

    def test_persona_with_empty_platform_context(self) -> None:
        """Given platform_context is explicitly empty string,
        When build_system_prompt() is called,
        Then platform context is not prepended (no leading newlines).
        """
        from agent_claude.persona import Persona

        persona = Persona(
            soul="soul",
            identity="identity",
            memory="memory",
            platform_context="",
        )
        prompt = persona.build_system_prompt()
        assert prompt.startswith("soul\n\nidentity\n\nmemory")


# ---------------------------------------------------------------------------
# Integration: assemble_platform_context with real layers
# ---------------------------------------------------------------------------

class TestAssemblePlatformContext:
    """Integration tests for assemble_platform_context()."""

    def test_all_layers_present(self, tmp_path: Path) -> None:
        """Given all three layers return content,
        When assemble_platform_context() is called,
        Then the result contains all three sections in order.
        """
        from agent_claude.layers import assemble_platform_context

        with patch(
            "agent_claude.layers.load_l0_capabilities",
            return_value="## Wheelhouse Capabilities\n\ncaps",
        ):
            with patch(
                "agent_claude.layers.load_l1_cli_reference",
                return_value="## CLI Reference\n\nref",
            ):
                with patch(
                    "agent_claude.layers.load_l2_topology_state",
                    return_value="## Topology State\n\ntopo",
                ):
                    result = assemble_platform_context()

        assert "## Wheelhouse Capabilities" in result
        assert "## CLI Reference" in result
        assert "## Topology State" in result
        assert result.index("Capabilities") < result.index("CLI Reference")
        assert result.index("CLI Reference") < result.index("Topology State")

    def test_only_l0_present(self) -> None:
        """Given only L0 returns content,
        When assemble_platform_context() is called,
        Then the result contains only L0 content.
        """
        from agent_claude.layers import assemble_platform_context

        with patch(
            "agent_claude.layers.load_l0_capabilities",
            return_value="## Wheelhouse Capabilities\n\ncaps",
        ):
            with patch(
                "agent_claude.layers.load_l1_cli_reference", return_value=None
            ):
                with patch(
                    "agent_claude.layers.load_l2_topology_state",
                    return_value=None,
                ):
                    result = assemble_platform_context()

        assert "## Wheelhouse Capabilities" in result
        assert "CLI Reference" not in result
        assert "Topology State" not in result
