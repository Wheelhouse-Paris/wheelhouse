"""Startup sequence orchestrator for agent-claude.

Startup order (ADR-018, ADR-033):
  1. Validate environment variables
  1b. Assemble platform context layers L0-L2 (ADR-033)
  2. Load persona files (L3)
  2b. Load stream contexts (L4)
  2c. Log total context size (E12-12)
  3. Connect to Wheelhouse broker via SDK
"""

from __future__ import annotations

import logging
import os
import sys
from typing import Any

import wheelhouse

from agent_claude.context import load_stream_contexts
from agent_claude.errors import AgentConfigError
from agent_claude.layers import assemble_platform_context
from agent_claude.persona import load_persona

logger = logging.getLogger("agent_claude")

# Default values per ADR-018
DEFAULT_PERSONA_PATH = "/persona"
DEFAULT_CONTEXT_PATH = "/context"


def validate_env() -> dict[str, Any]:
    """Validate all required environment variables.

    Required (ADR-018):
      - WH_URL
      - WH_AGENT_NAME
      - WH_STREAMS

    Optional with defaults:
      - WH_PERSONA_PATH (default: /persona)
      - WH_CONTEXT_PATH (default: /context) — mount point for .wh/context/

    Authentication is handled by the `claude` CLI (CLAUDE_CODE_OAUTH_TOKEN
    env var or credentials in ~/.claude/ inside the container).

    Returns:
        Configuration dict with parsed values.

    Raises:
        AgentConfigError: If any required variable is missing or empty.
    """
    wh_url = os.environ.get("WH_URL", "").strip()
    if not wh_url:
        raise AgentConfigError(
            "agent-claude: WH_URL is not set "
            "-- set it in the .wh topology file"
        )

    agent_name = os.environ.get("WH_AGENT_NAME", "").strip()
    if not agent_name:
        raise AgentConfigError(
            "agent-claude: WH_AGENT_NAME is not set "
            "-- set it in the .wh topology file"
        )

    streams_raw = os.environ.get("WH_STREAMS", "").strip()
    if not streams_raw:
        raise AgentConfigError(
            "agent-claude: WH_STREAMS is not set "
            "-- set it in the .wh topology file"
        )

    streams = [s.strip() for s in streams_raw.split(",") if s.strip()]
    if not streams:
        raise AgentConfigError(
            "agent-claude: WH_STREAMS is empty "
            "-- provide at least one stream name"
        )

    # Optional with defaults
    persona_path = os.environ.get("WH_PERSONA_PATH", DEFAULT_PERSONA_PATH).strip()
    context_path = os.environ.get("WH_CONTEXT_PATH", DEFAULT_CONTEXT_PATH).strip()

    return {
        "wh_url": wh_url,
        "agent_name": agent_name,
        "streams": streams,
        "persona_path": persona_path,
        "context_path": context_path,
    }


async def run_startup() -> dict[str, Any]:
    """Execute the full startup sequence.

    Order: validate_env -> assemble_platform_context (L0-L2)
           -> load_persona (L3) -> load_stream_contexts (L4)
           -> log total context size -> wheelhouse.connect

    Returns:
        Dict with 'config', 'persona', and 'connection' keys.

    Raises:
        AgentConfigError: If env validation fails.
        SystemExit: If connection to broker fails.
    """
    # Step 1: Validate environment
    config = validate_env()

    # Step 1b: Assemble platform context layers L0-L2 (ADR-033)
    platform_context = assemble_platform_context()

    # Step 2: Load persona (L3)
    persona = load_persona(config["persona_path"])
    persona.platform_context = platform_context

    # Step 2b: Load stream contexts (L4, ADR-021: once at startup, not per message)
    stream_contexts = load_stream_contexts(
        config["context_path"], config["streams"]
    )
    persona.stream_contexts = stream_contexts

    # Step 2c: Log total context size (E12-12)
    total_context = persona.build_system_prompt()
    total_size = len(total_context)
    logger.info(
        "Layered context assembled: %d total characters (L0-L4)",
        total_size,
    )

    # Step 3: Connect to Wheelhouse broker
    try:
        connection = await wheelhouse.connect(
            config["wh_url"],
            publisher_id=config["agent_name"],
        )
    except wheelhouse.ConnectionError as exc:
        logger.error("Failed to connect to broker at %s: %s", config["wh_url"], exc)
        sys.exit(1)

    logger.info(
        "agent-claude connected to broker at %s as %s",
        config["wh_url"],
        config["agent_name"],
    )

    return {
        "config": config,
        "persona": persona,
        "connection": connection,
    }
