"""Stream context loading for agent-claude.

Loads CONTEXT.md files from the context volume at startup.
Each stream may have a CONTEXT.md at <context_path>/<stream_name>/CONTEXT.md
(written by `wh deploy apply` from the stream's `description` field).

Per ADR-021, context files are loaded ONCE at startup and injected into
the system prompt. They are NOT re-read per message. Restart the container
to pick up changes.
"""

from __future__ import annotations

import logging
from pathlib import Path

logger = logging.getLogger("agent_claude")


def load_stream_contexts(
    context_path: str, streams: list[str]
) -> dict[str, str]:
    """Load CONTEXT.md files for each subscribed stream.

    Args:
        context_path: Root directory containing per-stream context subdirectories.
        streams: List of stream names to load context for.

    Returns:
        Dict mapping stream name -> CONTEXT.md content for streams that have one.
        Streams without a CONTEXT.md file are omitted.
    """
    base = Path(context_path)

    if not base.is_dir():
        logger.debug(
            "Context path %s does not exist — no stream contexts loaded",
            context_path,
        )
        return {}

    contexts: dict[str, str] = {}
    total_bytes = 0

    for stream_name in streams:
        context_file = base / stream_name / "CONTEXT.md"

        if not context_file.exists():
            logger.debug(
                "No CONTEXT.md for stream %s (looked at %s)",
                stream_name,
                context_file,
            )
            continue

        try:
            content = context_file.read_text(encoding="utf-8")
        except OSError as exc:
            logger.warning(
                "Failed to read CONTEXT.md for stream %s: %s",
                stream_name,
                exc,
            )
            continue

        byte_count = len(content.encode("utf-8"))
        total_bytes += byte_count
        contexts[stream_name] = content
        logger.debug(
            "Loaded CONTEXT.md for stream %s (%d bytes)",
            stream_name,
            byte_count,
        )

    if contexts:
        logger.info(
            "Loaded stream contexts: %d streams, %d total bytes",
            len(contexts),
            total_bytes,
        )
    else:
        logger.debug("No stream context files found")

    return contexts
