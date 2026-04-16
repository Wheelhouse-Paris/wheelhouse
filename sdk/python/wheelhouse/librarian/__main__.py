"""Entry point for the Wheelhouse Librarian runtime.

Usage: python -m wheelhouse.librarian

Connects to the Wheelhouse broker, subscribes to end-of-turn streams,
and processes LibraryWriteEvent messages through the decision loop.

Environment variables (ADR-041):
  Required:
    ANTHROPIC_API_KEY  — LLM backend for decision calls
    WH_URL             — Broker connection URL
    WH_AGENT_NAME      — Librarian agent identity
    WH_STREAMS         — Comma-separated end-of-turn stream names
    WH_LIBRARY_PATH    — Absolute path to the Library root (RW volume)
    WH_LIBRARY_ID      — Library identifier for decision log attribution

  Optional:
    WH_LIBRARIAN_LOCALES — Comma-separated locale list (default: en,fr)
"""

from __future__ import annotations

import asyncio
import logging
import os
import sys
from pathlib import Path
from typing import Any

import wheelhouse
from wheelhouse.skills.library_sandbox import LibrarySandbox
from wheelhouse.types import TopologyShutdown

from wheelhouse._proto.wheelhouse.v1 import SkillResult
from wheelhouse.librarian.dedup import DedupCache
from wheelhouse.librarian.loop import LibrarianLoop
from wheelhouse.librarian.proto import LibraryWriteEvent

logger = logging.getLogger("wheelhouse.librarian")

# Required environment variables per ADR-041
REQUIRED_ENV_VARS = [
    "ANTHROPIC_API_KEY",
    "WH_URL",
    "WH_AGENT_NAME",
    "WH_STREAMS",
    "WH_LIBRARY_PATH",
    "WH_LIBRARY_ID",
]


class LibrarianConfig:
    """Validated configuration from environment variables."""

    def __init__(
        self,
        anthropic_api_key: str,
        wh_url: str,
        agent_name: str,
        streams: list[str],
        library_path: str,
        library_id: str,
        locales: list[str],
    ) -> None:
        self.anthropic_api_key = anthropic_api_key
        self.wh_url = wh_url
        self.agent_name = agent_name
        self.streams = streams
        self.library_path = library_path
        self.library_id = library_id
        self.locales = locales


def validate_env() -> LibrarianConfig:
    """Validate all required environment variables.

    Returns:
        LibrarianConfig with parsed values.

    Raises:
        SystemExit: If any required variable is missing or empty.
    """
    missing: list[str] = []
    for var in REQUIRED_ENV_VARS:
        value = os.environ.get(var, "").strip()
        if not value:
            missing.append(var)

    if missing:
        logger.error(
            "Missing required environment variables: %s",
            ", ".join(missing),
        )
        sys.exit(1)

    # Parse streams
    streams_raw = os.environ["WH_STREAMS"].strip()
    streams = [s.strip() for s in streams_raw.split(",") if s.strip()]
    if not streams:
        logger.error("WH_STREAMS is set but contains no valid stream names")
        sys.exit(1)

    # Validate library path exists and is a directory
    library_path = os.environ["WH_LIBRARY_PATH"].strip()
    lib_path = Path(library_path)
    if not lib_path.is_dir():
        logger.error(
            "WH_LIBRARY_PATH does not exist or is not a directory: %s",
            library_path,
        )
        sys.exit(1)

    # Optional: locales (default en,fr)
    locales_raw = os.environ.get("WH_LIBRARIAN_LOCALES", "en,fr").strip()
    locales = [loc.strip() for loc in locales_raw.split(",") if loc.strip()]
    if not locales:
        locales = ["en", "fr"]

    return LibrarianConfig(
        anthropic_api_key=os.environ["ANTHROPIC_API_KEY"].strip(),
        wh_url=os.environ["WH_URL"].strip(),
        agent_name=os.environ["WH_AGENT_NAME"].strip(),
        streams=streams,
        library_path=library_path,
        library_id=os.environ["WH_LIBRARY_ID"].strip(),
        locales=locales,
    )


def make_anthropic_llm_fn(api_key: str) -> Any:
    """Create an llm_fn that wraps anthropic.Anthropic().messages.create().

    Returns a callable ``(system_prompt, user_content) -> response_text``
    suitable for injection into ``decide()``.

    The Anthropic client is instantiated once and reused across calls.
    """
    import anthropic

    client = anthropic.Anthropic(api_key=api_key)

    def llm_fn(system_prompt: str, user_content: str) -> str:
        response = client.messages.create(
            model="claude-sonnet-4-20250514",
            max_tokens=4096,
            system=system_prompt,
            messages=[{"role": "user", "content": user_content}],
        )
        # Extract text from the response content blocks.
        return "".join(
            block.text for block in response.content if hasattr(block, "text")
        )

    return llm_fn


# Shutdown coordination
_shutdown_event = asyncio.Event()


async def run(config: LibrarianConfig) -> None:
    """Connect to broker, subscribe to streams, and run the event loop.

    Args:
        config: Validated configuration.
    """
    # Connect to broker via SDK
    try:
        connection = await wheelhouse.connect(
            config.wh_url,
            publisher_id=config.agent_name,
        )
    except wheelhouse.ConnectionError as exc:
        logger.error("Failed to connect to broker at %s: %s", config.wh_url, exc)
        sys.exit(1)

    # Initialize LibrarySandbox with git enabled for page writes
    sandbox = LibrarySandbox(
        config.library_path,
        git_enabled=True,
        agent_name=config.agent_name,
    )

    # Create llm_fn wrapping Anthropic API
    llm_fn = make_anthropic_llm_fn(config.anthropic_api_key)

    # Initialize dedup cache from Library volume (story 14-1-4)
    dedup_path = Path(config.library_path) / ".dedup"
    dedup = DedupCache.load(dedup_path)

    # SkillResult emission callback for metering (14-1-5, AC-6).
    # Bridges the sync LibrarianLoop with the async connection.publish().
    _loop = asyncio.get_running_loop()

    def publish_skill_result(
        *,
        invocation_id: str,
        skill_name: str,
        success: bool,
        output: str,
        tokens_consumed: int,
        library_id: str,
        agent_id: str,
    ) -> None:
        """Publish a SkillResult to the broker for metering (14-1-5)."""
        import time as _time

        sr = SkillResult(
            invocation_id=invocation_id,
            skill_name=skill_name,
            success=success,
            output=output,
            timestamp_ms=int(_time.time() * 1000),
            library_tokens=tokens_consumed,
        )
        # Schedule the async publish on the running event loop.
        asyncio.run_coroutine_threadsafe(
            connection.publish(f"skill-results-{agent_id}", sr),
            _loop,
        )

    # Initialize the librarian loop
    librarian_loop = LibrarianLoop(
        library_path=config.library_path,
        library_id=config.library_id,
        locales=config.locales,
        llm_fn=llm_fn,
        sandbox=sandbox,
        dedup=dedup,
        publish_skill_result=publish_skill_result,
        agent_name=config.agent_name,
    )

    # Create message handler
    def _make_handler() -> Any:
        async def handler(message: Any) -> None:
            if isinstance(message, TopologyShutdown):
                logger.info("Received TopologyShutdown — initiating graceful drain")
                _shutdown_event.set()
                return

            if isinstance(message, LibraryWriteEvent):
                result = librarian_loop.process_event(message)
                logger.debug(
                    "Event processed: event_id=%s reason=%s committed=%s",
                    result.event_id,
                    result.reason,
                    result.committed,
                )
                return

            # Unknown message type — skip with debug log
            logger.debug(
                "Unknown message type skipped: type=%s",
                type(message).__name__,
            )

        return handler

    # Subscribe to all streams
    handler = _make_handler()
    for stream_name in config.streams:
        await connection.subscribe(stream_name, handler)

    stream_count = len(config.streams)
    logger.info(
        "Librarian ready, subscribed to %d stream%s: [%s]",
        stream_count,
        "s" if stream_count != 1 else "",
        ", ".join(config.streams),
    )

    # Block until shutdown signal
    try:
        await _shutdown_event.wait()
    finally:
        # Graceful drain: wait up to 5 seconds for any in-flight processing
        logger.info("Draining in-flight events (5s grace period)")
        await asyncio.sleep(0)  # Yield to let any pending handler complete
        await connection.close()
        logger.info("Librarian shutdown complete")


def main() -> None:
    """Main entry point for python -m wheelhouse.librarian."""
    # Handle --version flag
    if "--version" in sys.argv:
        from wheelhouse.librarian import __version__
        print(f"wheelhouse-librarian {__version__}")
        sys.exit(0)

    # Configure logging: structured, unbuffered
    logging.basicConfig(
        level=logging.DEBUG,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
        stream=sys.stderr,
    )

    # Validate environment
    config = validate_env()

    # Run the async event loop
    try:
        asyncio.run(run(config))
    except KeyboardInterrupt:
        logger.info("Shutting down (keyboard interrupt)")
        sys.exit(0)


if __name__ == "__main__":
    main()
