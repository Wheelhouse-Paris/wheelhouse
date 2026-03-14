"""Entry point for `python -m agent_claude`.

Supports --version flag for container version inspection (ADR-019).
Otherwise runs the full startup sequence.
"""

from __future__ import annotations

import asyncio
import logging
import sys

from agent_claude import _version_string
from agent_claude.errors import AgentConfigError, ClaudeAuthError


def main() -> None:
    """Main entry point."""
    # Handle --version flag (ADR-019)
    if "--version" in sys.argv:
        print(_version_string())
        sys.exit(0)

    # Configure logging: structured, unbuffered (PYTHONUNBUFFERED=1 in Dockerfile)
    logging.basicConfig(
        level=logging.DEBUG,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
        stream=sys.stderr,
    )

    try:
        asyncio.run(_run())
    except AgentConfigError as exc:
        logging.getLogger("agent_claude").error(str(exc))
        sys.exit(1)
    except ClaudeAuthError as exc:
        logging.getLogger("agent_claude").error(str(exc))
        sys.exit(1)
    except KeyboardInterrupt:
        logging.getLogger("agent_claude").info("Shutting down (keyboard interrupt)")
        sys.exit(0)


async def _run() -> None:
    """Async entry point -- runs startup then message processing loop (Story 8.2)."""
    from agent_claude.claude_client import ClaudeClient
    from agent_claude.loop import run_message_loop
    from agent_claude.main import run_startup

    result = await run_startup()

    connection = result["connection"]
    config = result["config"]
    persona = result["persona"]

    logger = logging.getLogger("agent_claude")
    logger.info(
        "Startup complete -- agent %s ready on streams: %s",
        config["agent_name"],
        ", ".join(config["streams"]),
    )

    # Initialize Claude Code CLI client
    claude_client = ClaudeClient()

    # Run message processing loop (blocks until shutdown)
    await run_message_loop(
        connection=connection,
        config=config,
        persona=persona,
        claude_client=claude_client,
    )


if __name__ == "__main__":
    main()
