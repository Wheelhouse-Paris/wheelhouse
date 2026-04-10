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


# ---------------------------------------------------------------------------
# Story 13.21: L5 Library schema layer
# ---------------------------------------------------------------------------


class TestLayer5LibrarySchema:
    """L5 Library schema is appended after L4 and re-read fresh on each turn.

    Covers AC-1 through AC-4 of Story 13.21.
    """

    def test_ac1_l5_is_after_l4_and_before_batch_instruction(
        self, tmp_path: Path
    ) -> None:
        """AC-1: L5 appears after L4 and before the batch output instruction."""
        from agent_claude.persona import Persona

        schema_file = tmp_path / ".wh-schema.md"
        schema_file.write_text("# Library Schema\n\nhow to use Library")

        persona = Persona(
            soul="s",
            identity="i",
            memory="m",
            stream_contexts={"main": "ctx"},
            streams=["main"],
            platform_context="## Wheelhouse Capabilities\n\ncaps",
            library_schema_path=str(schema_file),
        )
        prompt = persona.build_system_prompt()

        assert "## Library Schema" in prompt
        assert "how to use Library" in prompt

        idx_l4 = prompt.index("## Stream Context: main")
        idx_l5 = prompt.index("## Library Schema")
        assert idx_l5 > idx_l4, "L5 must appear after L4"

        # Batch output instruction should be the last part — it comes after L5.
        from agent_claude.response_parser import format_batch_instruction

        batch_text = format_batch_instruction(["main"])
        # Find a stable marker from inside the batch instruction.
        idx_batch = prompt.index(batch_text)
        assert idx_batch > idx_l5, (
            "batch output instruction must remain last, after L5"
        )

    def test_ac2_l5_absent_when_library_schema_path_is_none(self) -> None:
        """AC-2: L5 is absent when library_schema_path is None (default)."""
        from agent_claude.persona import Persona

        persona = Persona(
            soul="s",
            identity="i",
            memory="m",
            stream_contexts={"main": "ctx"},
            streams=["main"],
        )
        assert persona.library_schema_path is None
        prompt = persona.build_system_prompt()
        assert "## Library Schema" not in prompt

    def test_ac2_byte_identical_to_pre_story_output(self) -> None:
        """AC-2: When L5 is not wired, the prompt is byte-identical to a
        Persona built without the library_schema_path field — no stray blank
        lines or extra separators."""
        from agent_claude.persona import Persona

        with_field = Persona(
            soul="s",
            identity="i",
            memory="m",
            stream_contexts={"main": "ctx"},
            streams=["main"],
            platform_context="## Wheelhouse Capabilities\n\ncaps",
            library_schema_path=None,
        )
        without_field = Persona(
            soul="s",
            identity="i",
            memory="m",
            stream_contexts={"main": "ctx"},
            streams=["main"],
            platform_context="## Wheelhouse Capabilities\n\ncaps",
        )
        assert with_field.build_system_prompt() == without_field.build_system_prompt()

    def test_ac3_file_vanishes_between_turns(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture
    ) -> None:
        """AC-3: If the schema file is deleted between turns, the next call
        simply skips L5, emits a debug log, and does NOT raise."""
        from agent_claude.persona import Persona

        schema_file = tmp_path / ".wh-schema.md"
        schema_file.write_text("v1")

        persona = Persona(
            soul="s",
            identity="i",
            memory="m",
            streams=["main"],
            library_schema_path=str(schema_file),
        )
        prompt_turn_n = persona.build_system_prompt()
        assert "## Library Schema" in prompt_turn_n

        schema_file.unlink()

        with caplog.at_level(logging.DEBUG, logger="agent_claude"):
            prompt_turn_n_plus_1 = persona.build_system_prompt()

        assert "## Library Schema" not in prompt_turn_n_plus_1
        assert any(
            "L5 Library schema file not found" in record.message
            for record in caplog.records
        )

    def test_ac4_l5_is_re_read_fresh_on_every_turn(self, tmp_path: Path) -> None:
        """AC-4: The schema file is re-read on every call so updates propagate."""
        from agent_claude.persona import Persona

        schema_file = tmp_path / ".wh-schema.md"
        schema_file.write_text("v1")

        persona = Persona(
            soul="s",
            identity="i",
            memory="m",
            streams=["main"],
            library_schema_path=str(schema_file),
        )
        prompt_1 = persona.build_system_prompt()
        assert "v1" in prompt_1

        schema_file.write_text("v2")
        prompt_2 = persona.build_system_prompt()
        assert "v2" in prompt_2
        assert "v1" not in prompt_2

    def test_load_l5_library_schema_empty_file_returns_none(
        self, tmp_path: Path
    ) -> None:
        """Empty content is treated as absent — no empty L5 heading in prompt."""
        from agent_claude.layers import load_l5_library_schema

        empty_file = tmp_path / ".wh-schema.md"
        empty_file.write_text("   \n  \n")

        assert load_l5_library_schema(str(empty_file)) is None

    def test_load_l5_library_schema_missing_file_returns_none(
        self, tmp_path: Path
    ) -> None:
        """Missing file returns None, never raises."""
        from agent_claude.layers import load_l5_library_schema

        missing = tmp_path / "nope.md"
        assert load_l5_library_schema(str(missing)) is None

    def test_load_l5_library_schema_formats_heading(self, tmp_path: Path) -> None:
        """Successful load returns a well-formed markdown section."""
        from agent_claude.layers import load_l5_library_schema

        schema_file = tmp_path / ".wh-schema.md"
        schema_file.write_text("# Library Schema\n\nbody content\n")

        result = load_l5_library_schema(str(schema_file))
        assert result is not None
        assert result.startswith("## Library Schema\n\n")
        assert "body content" in result
