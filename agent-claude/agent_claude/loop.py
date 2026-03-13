"""Message dispatch and processing loop for agent-claude.

Subscribes to all declared streams and dispatches incoming messages
to the Claude API based on type (ADR-020 dispatch table).

Dispatch table:
  TextMessage           -> user turn prompt; call Claude API
  CronEvent             -> structured cron prompt; call Claude API
  SkillInvocation (us)  -> skill prompt + SkillProgress; call Claude API
  SkillInvocation (other) -> drop silently
  TopologyShutdown      -> graceful drain
  Any other type        -> log debug, skip
"""

from __future__ import annotations

import asyncio
import logging
from typing import Any

from wheelhouse.types import (
    CronEvent,
    SkillInvocation,
    SkillProgress,
    SkillResult,
    TextMessage,
    TopologyShutdown,
)

from agent_claude.claude_client import ClaudeClient
from agent_claude.persona import Persona

logger = logging.getLogger("agent_claude")

# Prompt templates per ADR-017
CRON_PROMPT_TEMPLATE = (
    "Scheduled cron job '{job_name}' triggered at {triggered_at} (UTC).\n"
    "Review your current context and determine what action to take."
)

SKILL_PROMPT_TEMPLATE = (
    "Skill '{skill_name}' has been invoked with the following input:\n\n"
    "{input_payload}\n\n"
    "Based on your persona and the skill definition, describe what you will do "
    "and provide the output."
)

# In-flight tasks for graceful shutdown (ADR-020)
_pending_tasks: set[asyncio.Task[Any]] = set()
_shutdown_event = asyncio.Event()


async def run_message_loop(
    connection: Any,
    config: dict[str, Any],
    persona: Persona,
    claude_client: ClaudeClient,
) -> None:
    """Subscribe to all declared streams and process incoming messages.

    This is the main message loop that blocks until shutdown (AC #1).

    Args:
        connection: The SDK Connection object.
        config: Configuration dict from validate_env().
        persona: Loaded Persona dataclass.
        claude_client: Initialized ClaudeClient.
    """
    agent_name = config["agent_name"]
    persona_path = config["persona_path"]
    streams = config["streams"]

    # Subscribe to all declared streams (AC #1)
    for stream_name in streams:
        handler = _make_handler(
            connection=connection,
            stream_name=stream_name,
            claude_client=claude_client,
            persona=persona,
            agent_name=agent_name,
            persona_path=persona_path,
        )
        await connection.subscribe(stream_name, handler)

    logger.info("subscribed to streams: [%s]", ", ".join(streams))

    # Block until shutdown signal
    try:
        await _shutdown_event.wait()
    finally:
        # Graceful drain: wait for in-flight tasks (ADR-020, 5s grace)
        if _pending_tasks:
            logger.info(
                "received TopologyShutdown -- draining %d in-flight calls",
                len(_pending_tasks),
            )
            done, pending = await asyncio.wait(_pending_tasks, timeout=5.0)
            for task in pending:
                task.cancel()
        await connection.close()


def _make_handler(
    connection: Any,
    stream_name: str,
    claude_client: ClaudeClient,
    persona: Persona,
    agent_name: str,
    persona_path: str,
) -> Any:
    """Create a message handler for a specific stream.

    The handler dispatches based on message type per ADR-020.
    """

    async def handler(message: Any) -> None:
        logger.debug(
            "Message received: type=%s stream=%s publisher=%s",
            type(message).__name__,
            stream_name,
            getattr(message, "publisher", "unknown"),
        )

        if isinstance(message, TopologyShutdown):
            logger.info("received TopologyShutdown -- draining in-flight calls")
            _shutdown_event.set()
            return

        if isinstance(message, TextMessage):
            await _handle_text_message(
                message, connection, stream_name, claude_client, persona,
                agent_name, persona_path,
            )
        elif isinstance(message, CronEvent):
            await _handle_cron_event(
                message, connection, stream_name, claude_client, persona,
                agent_name, persona_path,
            )
        elif isinstance(message, SkillInvocation):
            await _handle_skill_invocation(
                message, connection, stream_name, claude_client, persona,
                agent_name, persona_path,
            )
        else:
            logger.debug(
                "Unknown type skipped: type=%s stream=%s",
                type(message).__name__,
                stream_name,
            )

    return handler


async def _publish_response(
    connection: Any,
    stream_name: str,
    message: Any,
    log_context: str,
) -> None:
    """Publish a response message, catching and logging publish failures.

    Args:
        connection: The SDK Connection object.
        stream_name: The stream to publish to.
        message: The message object to publish (TextMessage or SkillResult).
        log_context: Description for logging (e.g. "type=TextMessage chars=42").
    """
    try:
        await connection.publish(stream_name, message)
        logger.info(
            "Response published: %s stream=%s",
            log_context,
            stream_name,
        )
    except Exception:
        logger.warning(
            "Failed to publish response: %s stream=%s",
            log_context,
            stream_name,
            exc_info=True,
        )


async def _handle_text_message(
    message: TextMessage,
    connection: Any,
    stream_name: str,
    claude_client: ClaudeClient,
    persona: Persona,
    agent_name: str,
    persona_path: str,
) -> None:
    """Handle an incoming TextMessage (AC #2, #6)."""
    # Self-echo filter (AC-07)
    if message.publisher == agent_name:
        logger.debug(
            "Self-message filtered: stream=%s publisher=%s",
            stream_name,
            message.publisher,
        )
        return

    # Re-read MEMORY.md before each Claude API call (AC-04)
    persona.reload_memory(persona_path)
    system_prompt = persona.build_system_prompt()

    task = asyncio.create_task(
        claude_client.complete(
            system_prompt=system_prompt,
            user_message=message.content,
            msg_type="TextMessage",
            stream_name=stream_name,
        )
    )
    _pending_tasks.add(task)
    task.add_done_callback(_pending_tasks.discard)
    result = await task
    if result is not None:
        response_msg = TextMessage(
            content=result.text,
            publisher=agent_name,
        )
        await _publish_response(
            connection,
            stream_name,
            response_msg,
            f"type=TextMessage chars={len(result.text)}",
        )


async def _handle_cron_event(
    message: CronEvent,
    connection: Any,
    stream_name: str,
    claude_client: ClaudeClient,
    persona: Persona,
    agent_name: str,
    persona_path: str,
) -> None:
    """Handle an incoming CronEvent (AC #3)."""
    # Re-read MEMORY.md before each Claude API call (AC-04)
    persona.reload_memory(persona_path)
    system_prompt = persona.build_system_prompt()

    user_message = CRON_PROMPT_TEMPLATE.format(
        job_name=message.job_name,
        triggered_at=message.triggered_at,
    )

    task = asyncio.create_task(
        claude_client.complete(
            system_prompt=system_prompt,
            user_message=user_message,
            msg_type="CronEvent",
            stream_name=stream_name,
        )
    )
    _pending_tasks.add(task)
    task.add_done_callback(_pending_tasks.discard)
    result = await task
    if result is not None:
        response_msg = TextMessage(
            content=result.text,
            publisher=agent_name,
        )
        await _publish_response(
            connection,
            stream_name,
            response_msg,
            f"type=TextMessage chars={len(result.text)}",
        )


async def _handle_skill_invocation(
    message: SkillInvocation,
    connection: Any,
    stream_name: str,
    claude_client: ClaudeClient,
    persona: Persona,
    agent_name: str,
    persona_path: str,
) -> None:
    """Handle an incoming SkillInvocation (AC #4, #7).

    Drops invocations addressed to other agents.
    Publishes SkillProgress within 2s before Claude API call (AC-06).
    """
    # Drop invocations addressed to other agents (exact match, case-sensitive)
    if message.target_agent != agent_name:
        logger.debug(
            "SkillInvocation dropped: target_agent=%s (not us: %s) stream=%s",
            message.target_agent,
            agent_name,
            stream_name,
        )
        return

    # Publish SkillProgress immediately (within 2s, AC-06)
    progress = SkillProgress(
        invocation_id=message.invocation_id,
        status="IN_PROGRESS",
        message="Processing...",
        publisher=agent_name,
    )
    await connection.publish(stream_name, progress)
    logger.debug(
        "SkillProgress published: invocation_id=%s stream=%s",
        message.invocation_id,
        stream_name,
    )

    # Re-read MEMORY.md before each Claude API call (AC-04)
    persona.reload_memory(persona_path)
    system_prompt = persona.build_system_prompt()

    user_message = SKILL_PROMPT_TEMPLATE.format(
        skill_name=message.skill_name,
        input_payload=message.input_payload,
    )

    task = asyncio.create_task(
        claude_client.complete(
            system_prompt=system_prompt,
            user_message=user_message,
            msg_type="SkillInvocation",
            stream_name=stream_name,
        )
    )
    _pending_tasks.add(task)
    task.add_done_callback(_pending_tasks.discard)
    result = await task
    if result is not None:
        skill_result = SkillResult(
            invocation_id=message.invocation_id,
            success=True,
            payload=result.text,
            publisher=agent_name,
        )
        await _publish_response(
            connection,
            stream_name,
            skill_result,
            f"type=SkillResult invocation_id={message.invocation_id} success=True",
        )
    else:
        # Publish failure SkillResult so the caller is not left hanging
        skill_result = SkillResult(
            invocation_id=message.invocation_id,
            success=False,
            payload="Claude API call failed",
            publisher=agent_name,
        )
        await _publish_response(
            connection,
            stream_name,
            skill_result,
            f"type=SkillResult invocation_id={message.invocation_id} success=False",
        )
