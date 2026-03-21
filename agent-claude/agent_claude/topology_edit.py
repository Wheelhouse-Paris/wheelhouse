"""Topology edit helpers for agents with topology_edit capability (ADR-034).

An agent with `topology_edit: true` in its `.wh` spec can create new `.wh`
files in the topology folder and run `wh topology apply` to extend the
running topology. This module provides helper functions for that pattern.

Prerequisites:
  - The agent must have `topology_edit: true` in its `.wh` spec (E12-13)
  - The `wh` binary must be available on PATH (injected in story 12-4)
  - The topology folder must be mounted/accessible inside the container

Usage pattern:
  1. Agent decides to extend the topology (e.g., spawn a sub-agent)
  2. Agent creates a `.wh` file describing the new components
  3. Agent runs `wh topology plan --format json` to preview changes
  4. Agent runs `wh topology apply --yes --agent-name <name>` to apply

If the agent does not have `topology_edit: true`, the apply command
exits with code 1 and an error message.
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
from pathlib import Path
from typing import Any

logger = logging.getLogger("agent_claude")


def create_wh_file(
    topology_dir: str,
    filename: str,
    content: str,
) -> Path:
    """Write a `.wh` file into the topology folder.

    Args:
        topology_dir: Path to the topology folder.
        filename: Name for the `.wh` file (e.g., "sub-agent.wh").
        content: YAML content for the `.wh` file.

    Returns:
        Path to the created file.

    Raises:
        OSError: If the file cannot be written.
    """
    path = Path(topology_dir) / filename
    path.write_text(content, encoding="utf-8")
    logger.info("Created .wh file: %s", path)
    return path


def topology_plan(
    wh_file: str | Path,
    *,
    timeout: int = 30,
) -> dict[str, Any]:
    """Run `wh topology plan --format json` and return parsed output.

    Args:
        wh_file: Path to the `.wh` file to plan.
        timeout: Subprocess timeout in seconds.

    Returns:
        Parsed JSON output from `wh topology plan`.

    Raises:
        subprocess.TimeoutExpired: If the command times out.
        subprocess.CalledProcessError: If the command fails with an unexpected error.
        json.JSONDecodeError: If the output is not valid JSON.
    """
    result = subprocess.run(
        ["wh", "topology", "plan", str(wh_file), "--format", "json"],
        capture_output=True,
        text=True,
        timeout=timeout,
    )

    # Exit code 0 = no changes, exit code 2 = changes detected — both are valid
    if result.returncode not in (0, 2):
        logger.error(
            "wh topology plan failed (exit %d): %s",
            result.returncode,
            result.stderr.strip(),
        )
        raise subprocess.CalledProcessError(
            result.returncode,
            result.args,
            result.stdout,
            result.stderr,
        )

    return json.loads(result.stdout)


def topology_apply(
    wh_file: str | Path,
    agent_name: str | None = None,
    *,
    timeout: int = 120,
) -> dict[str, Any] | None:
    """Run `wh topology apply --yes` and return the result.

    If `agent_name` is not provided, it defaults to the `WH_AGENT_NAME`
    environment variable (set automatically inside agent containers).

    Args:
        wh_file: Path to the `.wh` file to apply.
        agent_name: Agent name for attribution and permission check.
        timeout: Subprocess timeout in seconds.

    Returns:
        Parsed JSON output on success, None on failure.
    """
    if agent_name is None:
        agent_name = os.environ.get("WH_AGENT_NAME", "")

    cmd = [
        "wh",
        "topology",
        "apply",
        str(wh_file),
        "--yes",
        "--format",
        "json",
    ]
    if agent_name:
        cmd.extend(["--agent-name", agent_name])

    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=timeout,
    )

    if result.returncode != 0:
        logger.error(
            "wh topology apply failed (exit %d): %s",
            result.returncode,
            result.stderr.strip(),
        )
        return None

    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        logger.warning(
            "wh topology apply succeeded but output was not JSON: %s",
            result.stdout[:200],
        )
        return {"applied": True}
