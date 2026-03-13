"""Entry point for `python -m agent_claude`.

Supports --version flag for container version inspection (ADR-019).
Otherwise runs the full startup sequence.
"""

from __future__ import annotations

import asyncio
import logging
import sys

from agent_claude import _version_string
from agent_claude.errors import AgentConfigError


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
    except KeyboardInterrupt:
        logging.getLogger("agent_claude").info("Shutting down (keyboard interrupt)")
        sys.exit(0)


async def _run() -> None:
    """Async entry point — runs startup and then the message loop (Story 8.2)."""
    from agent_claude.main import run_startup

    result = await run_startup()

    # Message loop will be implemented in Story 8.2.
    # For now, the startup sequence is complete and the connection is established.
    connection = result["connection"]
    config = result["config"]

    logger = logging.getLogger("agent_claude")
    logger.info(
        "Startup complete — agent %s ready on streams: %s",
        config["agent_name"],
        ", ".join(config["streams"]),
    )

    # Placeholder: Story 8.2 will add the message processing loop here.
    # For now, we keep the connection open until shutdown.
    try:
        # Block until interrupted
        await asyncio.Event().wait()
    finally:
        await connection.close()


if __name__ == "__main__":
    main()
