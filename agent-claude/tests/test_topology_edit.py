"""Tests for agent_claude.topology_edit module (ADR-034)."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from unittest.mock import patch

import pytest

from agent_claude.topology_edit import (
    create_wh_file,
    topology_apply,
    topology_plan,
)


class TestCreateWhFile:
    """Tests for create_wh_file helper."""

    def test_creates_file_at_correct_path(self, tmp_path: Path) -> None:
        content = "apiVersion: wheelhouse.dev/v1\nname: test\n"
        result = create_wh_file(str(tmp_path), "sub-agent.wh", content)

        assert result == tmp_path / "sub-agent.wh"
        assert result.read_text() == content

    def test_overwrites_existing_file(self, tmp_path: Path) -> None:
        path = tmp_path / "existing.wh"
        path.write_text("old content")

        create_wh_file(str(tmp_path), "existing.wh", "new content")
        assert path.read_text() == "new content"


class TestTopologyPlan:
    """Tests for topology_plan helper."""

    def test_parses_json_output_on_exit_0(self, tmp_path: Path) -> None:
        wh_file = tmp_path / "test.wh"
        wh_file.write_text("dummy")

        plan_output = {"v": 1, "status": "ok", "data": {"has_changes": False}}

        with patch("subprocess.run") as mock_run:
            mock_run.return_value.returncode = 0
            mock_run.return_value.stdout = json.dumps(plan_output)
            mock_run.return_value.stderr = ""

            result = topology_plan(wh_file)

        assert result["data"]["has_changes"] is False

    def test_parses_json_output_on_exit_2(self, tmp_path: Path) -> None:
        wh_file = tmp_path / "test.wh"
        wh_file.write_text("dummy")

        plan_output = {"v": 1, "status": "ok", "data": {"has_changes": True}}

        with patch("subprocess.run") as mock_run:
            mock_run.return_value.returncode = 2
            mock_run.return_value.stdout = json.dumps(plan_output)
            mock_run.return_value.stderr = ""

            result = topology_plan(wh_file)

        assert result["data"]["has_changes"] is True

    def test_raises_on_exit_1(self, tmp_path: Path) -> None:
        wh_file = tmp_path / "test.wh"
        wh_file.write_text("dummy")

        with patch("subprocess.run") as mock_run:
            mock_run.return_value.returncode = 1
            mock_run.return_value.stdout = ""
            mock_run.return_value.stderr = "error: invalid topology"
            mock_run.return_value.args = ["wh", "topology", "plan"]

            with pytest.raises(Exception):
                topology_plan(wh_file)


class TestTopologyApply:
    """Tests for topology_apply helper."""

    def test_success_returns_json(self, tmp_path: Path) -> None:
        wh_file = tmp_path / "test.wh"
        wh_file.write_text("dummy")

        apply_output = {"v": 1, "status": "ok", "data": {"applied": True}}

        with patch("subprocess.run") as mock_run:
            mock_run.return_value.returncode = 0
            mock_run.return_value.stdout = json.dumps(apply_output)
            mock_run.return_value.stderr = ""

            result = topology_apply(wh_file, agent_name="donna")

        assert result is not None
        assert result["data"]["applied"] is True
        # Verify --agent-name was passed
        cmd = mock_run.call_args[0][0]
        assert "--agent-name" in cmd
        assert "donna" in cmd

    def test_failure_returns_none(self, tmp_path: Path) -> None:
        wh_file = tmp_path / "test.wh"
        wh_file.write_text("dummy")

        with patch("subprocess.run") as mock_run:
            mock_run.return_value.returncode = 1
            mock_run.return_value.stdout = ""
            mock_run.return_value.stderr = "agent 'researcher' does not have topology_edit capability"

            result = topology_apply(wh_file, agent_name="researcher")

        assert result is None

    def test_defaults_to_wh_agent_name_env(self, tmp_path: Path) -> None:
        wh_file = tmp_path / "test.wh"
        wh_file.write_text("dummy")

        with patch("subprocess.run") as mock_run, \
             patch.dict(os.environ, {"WH_AGENT_NAME": "donna"}):
            mock_run.return_value.returncode = 0
            mock_run.return_value.stdout = '{"applied": true}'
            mock_run.return_value.stderr = ""

            topology_apply(wh_file)

        cmd = mock_run.call_args[0][0]
        assert "--agent-name" in cmd
        assert "donna" in cmd
