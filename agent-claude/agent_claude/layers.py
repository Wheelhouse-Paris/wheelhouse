"""Layered context assembly for agent-claude (ADR-033, Story 12.4).

Assembles startup context in 5 layers, each additive, in fixed order:

  L0: /etc/wh/capabilities.json — what Wheelhouse can do
  L1: /etc/wh/cli-reference.md (or `wh reference` subprocess) — how to invoke wh commands
  L2: `wh topology plan --format json` subprocess — current topology state
  L3: /persona/{SOUL,IDENTITY,MEMORY}.md — agent identity (existing, handled by persona.py)
  L4: .wh/context/<stream>/CONTEXT.md — per-stream context (existing, handled by context.py)

Constraints:
  E12-10: Layer assembly order L0→L1→L2→L3→L4 is fixed
  E12-11: Missing layers are skipped with a warning log — agent still starts
  E12-12: Total context size logged at startup for observability
"""

from __future__ import annotations

import json
import logging
import subprocess
from pathlib import Path

logger = logging.getLogger("agent_claude")

# Default path for capabilities manifest (baked into container image)
DEFAULT_CAPABILITIES_PATH = "/etc/wh/capabilities.json"

# Default path for CLI reference (baked into container image)
DEFAULT_CLI_REFERENCE_PATH = "/etc/wh/cli-reference.md"

# Subprocess timeouts
_WH_REFERENCE_TIMEOUT = 5.0
_WH_TOPOLOGY_TIMEOUT = 10.0


def load_l0_capabilities(path: str = DEFAULT_CAPABILITIES_PATH) -> str | None:
    """Load L0: Wheelhouse capabilities manifest.

    Reads /etc/wh/capabilities.json and formats it as a context section.

    Args:
        path: Path to capabilities.json file.

    Returns:
        Formatted context section string, or None if unavailable.
    """
    capabilities_path = Path(path)

    if not capabilities_path.exists():
        logger.warning(
            "L0 capabilities.json not found at %s — skipping layer", path
        )
        return None

    try:
        raw = capabilities_path.read_text(encoding="utf-8")
    except OSError as exc:
        logger.warning(
            "L0 capabilities.json unreadable at %s: %s — skipping layer",
            path,
            exc,
        )
        return None

    # Validate it's valid JSON
    try:
        json.loads(raw)
    except json.JSONDecodeError as exc:
        logger.warning(
            "L0 capabilities.json is not valid JSON: %s — skipping layer", exc
        )
        return None

    return f"## Wheelhouse Capabilities\n\n```json\n{raw.strip()}\n```"


def load_l1_cli_reference(
    file_path: str = DEFAULT_CLI_REFERENCE_PATH,
) -> str | None:
    """Load L1: CLI reference.

    First tries to read /etc/wh/cli-reference.md (baked into image).
    Falls back to running `wh reference` subprocess.

    Args:
        file_path: Path to cli-reference.md file.

    Returns:
        Formatted context section string, or None if unavailable.
    """
    # Try reading baked-in file first
    ref_path = Path(file_path)
    if ref_path.exists():
        try:
            content = ref_path.read_text(encoding="utf-8")
            if content.strip():
                logger.debug(
                    "L1 CLI reference loaded from file %s (%d bytes)",
                    file_path,
                    len(content.encode("utf-8")),
                )
                return f"## CLI Reference\n\n{content.strip()}"
        except OSError as exc:
            logger.debug(
                "L1 CLI reference file unreadable: %s — trying subprocess",
                exc,
            )

    # Fall back to subprocess
    try:
        proc = subprocess.run(
            ["wh", "reference"],
            capture_output=True,
            text=True,
            timeout=_WH_REFERENCE_TIMEOUT,
        )
    except FileNotFoundError:
        logger.warning(
            "L1 wh binary not found on PATH — skipping CLI reference layer"
        )
        return None
    except subprocess.TimeoutExpired:
        logger.warning(
            "L1 wh reference timed out after %.0fs — skipping layer",
            _WH_REFERENCE_TIMEOUT,
        )
        return None

    if proc.returncode != 0:
        logger.warning(
            "L1 wh reference failed (exit %d): %s — skipping layer",
            proc.returncode,
            proc.stderr.strip()[:200],
        )
        return None

    output = proc.stdout.strip()
    if not output:
        logger.warning("L1 wh reference returned empty output — skipping layer")
        return None

    logger.debug(
        "L1 CLI reference loaded from subprocess (%d bytes)",
        len(output.encode("utf-8")),
    )
    return f"## CLI Reference\n\n{output}"


def load_l2_topology_state() -> str | None:
    """Load L2: Current topology state.

    Runs `wh topology plan --format json` as a subprocess.

    Returns:
        Formatted context section string, or None if unavailable.
    """
    try:
        proc = subprocess.run(
            ["wh", "topology", "plan", "--format", "json"],
            capture_output=True,
            text=True,
            timeout=_WH_TOPOLOGY_TIMEOUT,
        )
    except FileNotFoundError:
        logger.warning(
            "L2 wh binary not found on PATH — skipping topology state layer"
        )
        return None
    except subprocess.TimeoutExpired:
        logger.warning(
            "L2 wh topology plan timed out after %.0fs — skipping layer",
            _WH_TOPOLOGY_TIMEOUT,
        )
        return None

    if proc.returncode != 0:
        logger.warning(
            "L2 wh topology plan failed (exit %d): %s — skipping layer",
            proc.returncode,
            proc.stderr.strip()[:200],
        )
        return None

    output = proc.stdout.strip()
    if not output:
        logger.warning(
            "L2 wh topology plan returned empty output — skipping layer"
        )
        return None

    # Validate it's valid JSON
    try:
        json.loads(output)
    except json.JSONDecodeError as exc:
        logger.warning(
            "L2 wh topology plan output is not valid JSON: %s — skipping layer",
            exc,
        )
        return None

    logger.debug(
        "L2 topology state loaded (%d bytes)",
        len(output.encode("utf-8")),
    )
    return f"## Topology State\n\n```json\n{output}\n```"


def assemble_platform_context() -> str:
    """Assemble L0, L1, L2 platform context layers.

    Layers are assembled in fixed order (E12-10). Missing layers
    are skipped with a warning (E12-11).

    Returns:
        Combined platform context string (may be empty if all layers missing).
    """
    sections: list[str] = []

    l0 = load_l0_capabilities()
    if l0 is not None:
        sections.append(l0)

    l1 = load_l1_cli_reference()
    if l1 is not None:
        sections.append(l1)

    l2 = load_l2_topology_state()
    if l2 is not None:
        sections.append(l2)

    return "\n\n".join(sections)
