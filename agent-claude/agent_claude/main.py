"""Startup sequence orchestrator for agent-claude.

Startup order (ADR-018):
  1. Validate environment variables
  2. Load persona files
  3. Connect to Wheelhouse broker via SDK
"""

from __future__ import annotations

import logging
import os
import sys
from typing import Any

import wheelhouse

from agent_claude.errors import AgentConfigError
from agent_claude.persona import load_persona

logger = logging.getLogger("agent_claude")

# Default values per ADR-018
DEFAULT_PERSONA_PATH = "/persona"
DEFAULT_MODEL = "claude-3-5-sonnet-20241022"


def validate_env() -> dict[str, Any]:
    """Validate all required environment variables.

    Required (ADR-018):
      - ANTHROPIC_API_KEY
      - WH_URL
      - WH_AGENT_NAME
      - WH_STREAMS

    Optional with defaults:
      - WH_PERSONA_PATH (default: /persona)
      - CLAUDE_MODEL (default: claude-3-5-sonnet-20241022)

    Returns:
        Configuration dict with parsed values.

    Raises:
        AgentConfigError: If any required variable is missing or empty.
    """
    # Check required vars -- absent OR empty both fail
    api_key = os.environ.get("ANTHROPIC_API_KEY", "").strip()
    if not api_key:
        raise AgentConfigError(
            "agent-claude: ANTHROPIC_API_KEY is not set "
            "-- set it via 'wh secrets init' or the ANTHROPIC_API_KEY environment variable"
        )

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
    model = os.environ.get("CLAUDE_MODEL", DEFAULT_MODEL).strip()

    return {
        "api_key": api_key,
        "wh_url": wh_url,
        "agent_name": agent_name,
        "streams": streams,
        "persona_path": persona_path,
        "model": model,
    }


async def run_startup() -> dict[str, Any]:
    """Execute the full startup sequence.

    Order: validate_env -> load_persona -> wheelhouse.connect

    Returns:
        Dict with 'config', 'persona', and 'connection' keys.

    Raises:
        AgentConfigError: If env validation fails.
        SystemExit: If connection to broker fails.
    """
    # Step 1: Validate environment
    config = validate_env()

    # Step 2: Load persona
    persona = load_persona(config["persona_path"])

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
