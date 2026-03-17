"""Batch publisher for agent-claude (ADR-022).

Publishes parsed batch response items to their target streams.
Each item becomes a TextMessage published to the specified stream.
"""

from __future__ import annotations

import logging
from typing import Any

from wheelhouse.types import TextMessage

logger = logging.getLogger("agent_claude")


async def publish_batch(
    connection: Any,
    items: list[dict],
    agent_name: str,
    source_stream: str,
    reply_to_user_id: str | None = None,
) -> None:
    """Publish batch response items to their target streams.

    For each item, creates a TextMessage and publishes it to the
    specified stream. Publish failures on individual items are logged
    as warnings but do not abort remaining items.

    Args:
        connection: The SDK Connection object.
        items: Parsed batch items, each with 'stream', 'type', 'content'.
        agent_name: The agent's publisher_id.
        source_stream: The stream the original message came from.
            Used to determine reply_to_user_id routing (AC-7).
        reply_to_user_id: User ID for reply routing. Set on messages
            targeting the source_stream only; omitted for cross-stream
            publishes.
    """
    if not items:
        return

    published = 0
    streams_targeted: set[str] = set()

    for item in items:
        target_stream = item["stream"]
        content = item["content"]

        # Build TextMessage — set reply_to_user_id only for source stream (AC-7)
        msg_kwargs: dict[str, Any] = {
            "content": content,
            "publisher_id": agent_name,
        }
        if target_stream == source_stream and reply_to_user_id is not None:
            msg_kwargs["reply_to_user_id"] = reply_to_user_id

        message = TextMessage(**msg_kwargs)

        try:
            await connection.publish(target_stream, message)
            published += 1
            streams_targeted.add(target_stream)
            logger.debug(
                "Batch item published: stream=%s chars=%d",
                target_stream,
                len(content),
            )
        except Exception:
            logger.warning(
                "Failed to publish batch item: stream=%s chars=%d",
                target_stream,
                len(content),
                exc_info=True,
            )

    logger.info(
        "Batch publish complete: %d/%d items published to streams [%s]",
        published,
        len(items),
        ", ".join(sorted(streams_targeted)),
    )
