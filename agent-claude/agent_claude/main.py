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
from agent_claude.library import build_library_sandbox
from agent_claude.persona import load_persona

logger = logging.getLogger("agent_claude")

# Default values per ADR-018
DEFAULT_PERSONA_PATH = "/persona"
DEFAULT_CONTEXT_PATH = "/context"

# Library schema file path (Epic 13 story 13-20, ADR-036, ADR-037).
# The schema file is materialized at /workspace/.wh-schema.md (workspace root,
# NOT inside .library/) by wh-broker's populate_workspace_volumes step before
# the agent container starts. The agent reaches it as `./wh-schema.md` from
# cwd=/workspace/ (set in claude_client.py per ADR-036, story 13-2).
LIBRARY_SCHEMA_PATH = "/workspace/.wh-schema.md"


def check_library_schema() -> str:
    """Check whether the Library schema file is present and readable.

    Returns ``"enabled"`` if ``/workspace/.wh-schema.md`` exists and is
    readable, else ``"disabled"`` (and emits a WARNING log line).

    NFR24 graceful absence (Epic 13 story 13-20, ADR-037): if the schema
    file is missing or unreadable at boot, the agent does NOT crash. It
    continues to start normally, but Library skills will refuse to operate
    because the schema (the agent's instructions on how to use the Library)
    is the prerequisite. This handles volume corruption, manual deletion,
    and the brief window between agent start and the first successful
    schema-write by ``populate_workspace_volumes``.

    The function only inspects the filesystem; it does not write to
    ``os.environ`` or any other side channel. ``run_startup`` is responsible
    for env-var propagation, with cloud-side precedence rules.
    """
    if os.path.isfile(LIBRARY_SCHEMA_PATH) and os.access(LIBRARY_SCHEMA_PATH, os.R_OK):
        logger.debug("Library schema file present at %s", LIBRARY_SCHEMA_PATH)
        return "enabled"
    logger.warning(
        "Library disabled — schema file missing or unreadable at %s "
        "(NFR24 graceful absence; agent will boot normally but Library "
        "skills will refuse to operate)",
        LIBRARY_SCHEMA_PATH,
    )
    return "disabled"


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

    # Step 1a: Probe Library schema file (NFR24 graceful absence; story 13-20).
    # If the file is missing the agent boots with Library disabled — it does
    # NOT crash and does NOT block the rest of startup. Cloud-side precedence:
    # if WH_LIBRARY_STATUS is already set (by the cloud provisioner with values
    # like "active" / "read-only" per the user's billing plan), the agent does
    # NOT overwrite it. The agent only writes "disabled" when the file is
    # absent AND no cloud value is present.
    library_status = check_library_schema()
    config["library_status"] = library_status
    if library_status == "disabled" and "WH_LIBRARY_STATUS" not in os.environ:
        os.environ["WH_LIBRARY_STATUS"] = "disabled"

    # Story 13-7: fold the cloud-side WH_LIBRARY_STATUS into the
    # runtime vocabulary. 13-20's boot probe only distinguishes
    # "enabled" vs "disabled" from the filesystem; the cloud may have
    # set `WH_LIBRARY_STATUS=read-only` via the agent container env
    # (ADR-038 Cross-Codebase Contract §3), and the ingest skill needs
    # to see that value on `config["library_status"]` to surface the
    # FR37 refusal. Map the three accepted env values onto the
    # internal three-value vocabulary; unknown values fall through to
    # the schema-probe result (no silent promotion).
    env_status = os.environ.get("WH_LIBRARY_STATUS", "").strip()
    if env_status == "active":
        # Cloud "active" is the same as framework "enabled".
        config["library_status"] = "enabled"
    elif env_status in ("enabled", "read-only", "disabled"):
        config["library_status"] = env_status

    # Step 1c: Build the per-agent LibrarySandbox (story 13-7). Runs
    # `recover_from_crash()` exactly once before any skill dispatch can
    # see an unrecovered repo. Returns None when the Library is
    # disabled or recovery fails — the dispatch layer degrades to a
    # clean LIBRARY_DISABLED refusal in that case.
    config["library_sandbox"] = build_library_sandbox(config)

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
