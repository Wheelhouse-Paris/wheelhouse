"""End-of-turn LibraryWriteEvent emission for client agents (ADR-042).

When a client agent is a member of an llm-wiki subsystem, the composition
loader sets WH_LIBRARY_WRITE_STREAM and WH_LIBRARY_ID env vars. This module
checks for those vars and, when present, publishes a LibraryWriteEvent after
each assistant response.

Emission is transparent to the agent code (FR26, FR27): no agent code changes
required.
"""

from __future__ import annotations

import logging
import os
import time
import uuid
from typing import Any

from wheelhouse._proto.wheelhouse.librarian.v1 import (
    ConversationMessage,
    LibraryWriteEvent,
)
from wheelhouse.librarian.locale import detect_locale

logger = logging.getLogger("wheelhouse.librarian")

# Cache env var lookups at module level; rechecked per call for testability.
_STREAM_ENV = "WH_LIBRARY_WRITE_STREAM"
_LIBRARY_ID_ENV = "WH_LIBRARY_ID"


async def maybe_emit_write_event(
    connection: Any,
    agent_name: str,
    user_message: str,
    assistant_response: str,
    conversation_id: str,
) -> None:
    """Emit a LibraryWriteEvent if the agent is configured for librarian emission.

    This function is safe to call unconditionally. If the env vars are not set,
    it returns immediately with zero overhead. On publish failure, it logs a
    warning and returns (never blocks the agent response path).

    Args:
        connection: The SDK Connection object for publishing.
        agent_name: The agent's name (source_agent_id).
        user_message: The user's input message content.
        assistant_response: The assistant's response content.
        conversation_id: Conversation session identifier (user_id or stream).
    """
    stream_name = os.environ.get(_STREAM_ENV)
    library_id = os.environ.get(_LIBRARY_ID_ENV)

    if not stream_name or not library_id:
        return

    now_ms = int(time.time() * 1000)

    # Detect locale from the combined conversation text
    combined_text = f"{user_message} {assistant_response}"
    locale = detect_locale(combined_text)

    event = LibraryWriteEvent(
        event_id=str(uuid.uuid4()),
        source_agent_id=agent_name,
        library_id=library_id,
        conversation_id=conversation_id,
        timestamp_ms=now_ms,
        segment=[
            ConversationMessage(
                role="user",
                content=user_message,
                timestamp_ms=now_ms,
            ),
            ConversationMessage(
                role="assistant",
                content=assistant_response,
                timestamp_ms=now_ms,
            ),
        ],
        locale=locale,
    )

    try:
        await connection.publish(stream_name, event)
        logger.debug(
            "LibraryWriteEvent emitted: event_id=%s stream=%s locale=%s",
            event.event_id,
            stream_name,
            locale or "(empty)",
        )
    except Exception:
        logger.warning(
            "Failed to emit LibraryWriteEvent: stream=%s",
            stream_name,
            exc_info=True,
        )
