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
import json
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

from agent_claude.batch_publisher import publish_batch
from agent_claude.claude_client import ClaudeClient
from agent_claude.errors import ClaudeAuthError
from agent_claude.persona import Persona
from agent_claude.response_parser import parse_batch_response

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

# Fatal error storage: when a ClaudeAuthError is raised inside a handler,
# the SDK's _listen() loop catches all exceptions. We store the error here
# and set _shutdown_event so run_message_loop() can re-raise it (AC-02).
_fatal_error: ClaudeAuthError | None = None


async def run_message_loop(
    connection: Any,
    config: dict[str, Any],
    persona: Persona,
    claude_client: ClaudeClient,
) -> None:
    """Subscribe to all declared streams and process incoming messages.

    This is the main message loop that blocks until shutdown (AC #1).

    If a ClaudeAuthError occurs during message processing, the handler stores
    it in _fatal_error and sets the shutdown event. After draining, this
    function re-raises the error so __main__.py can catch it and exit(1).

    Args:
        connection: The SDK Connection object.
        config: Configuration dict from validate_env().
        persona: Loaded Persona dataclass.
        claude_client: Initialized ClaudeClient.

    Raises:
        ClaudeAuthError: If the API key is invalid (AC-02, FR-AC6).
    """
    global _fatal_error
    _fatal_error = None
    _shutdown_event.clear()

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

    # Re-raise fatal error after cleanup (AC-02)
    if _fatal_error is not None:
        raise _fatal_error


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
        global _fatal_error
        logger.debug(
            "Message received: type=%s stream=%s publisher=%s",
            type(message).__name__,
            stream_name,
            getattr(message, "publisher_id", "unknown"),
        )

        if isinstance(message, TopologyShutdown):
            logger.info("received TopologyShutdown -- draining in-flight calls")
            _shutdown_event.set()
            return

        try:
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
        except ClaudeAuthError as exc:
            # Fatal: invalid API key. Store error and signal shutdown so
            # run_message_loop() can re-raise after cleanup (AC-02).
            _fatal_error = exc
            _shutdown_event.set()
            raise  # Re-raise so SDK also logs it

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
    # Self-echo filter (AC-07): skip messages published by this agent
    if message.publisher_id == agent_name:
        logger.debug(
            "Self-message filtered: stream=%s publisher_id=%s",
            stream_name,
            message.publisher_id,
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
            conversation_id=message.user_id or stream_name,
        )
    )
    _pending_tasks.add(task)
    task.add_done_callback(_pending_tasks.discard)
    result = await task
    if result is not None:
        # Parse batch response (ADR-022)
        items = parse_batch_response(result.text)
        if items is None:
            logger.error(
                "Malformed batch response from Claude: stream=%s",
                stream_name,
            )
            return
        if not items:
            logger.debug("Empty batch response (no-op): stream=%s", stream_name)
            return
        await publish_batch(
            connection,
            items,
            agent_name,
            source_stream=stream_name,
            reply_to_user_id=message.user_id,
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
        triggered_at=str(message.triggered_at),
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
        # Parse batch response (ADR-022)
        items = parse_batch_response(result.text)
        if items is None:
            logger.error(
                "Malformed batch response from Claude: stream=%s type=CronEvent",
                stream_name,
            )
            return
        if not items:
            logger.debug(
                "Empty batch response (no-op): stream=%s type=CronEvent",
                stream_name,
            )
            return
        await publish_batch(
            connection,
            items,
            agent_name,
            source_stream=stream_name,
            reply_to_user_id=None,
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
    if message.agent_id != agent_name:
        logger.debug(
            "SkillInvocation dropped: agent_id=%s (not us: %s) stream=%s",
            message.agent_id,
            agent_name,
            stream_name,
        )
        return

    # Publish SkillProgress immediately (within 2s, AC-06)
    progress = SkillProgress(
        invocation_id=message.invocation_id,
        skill_name=message.skill_name,
        status_message="Processing...",
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

    # Format parameters dict as JSON string for the prompt
    input_payload = json.dumps(dict(message.parameters)) if message.parameters else "{}"

    user_message = SKILL_PROMPT_TEMPLATE.format(
        skill_name=message.skill_name,
        input_payload=input_payload,
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
        # Parse batch response for side-effect TextMessage publishes (ADR-022)
        items = parse_batch_response(result.text)
        if items is None:
            # Malformed batch: publish SkillResult(success=False)
            logger.error(
                "Malformed batch response from Claude: stream=%s type=SkillInvocation",
                stream_name,
            )
            fail_result = SkillResult(
                invocation_id=message.invocation_id,
                skill_name=message.skill_name,
                success=False,
                error_message="Malformed batch response from Claude",
            )
            await _publish_response(
                connection,
                stream_name,
                fail_result,
                f"type=SkillResult invocation_id={message.invocation_id} success=False",
            )
            return

        # Publish any batch TextMessage items
        if items:
            await publish_batch(
                connection,
                items,
                agent_name,
                source_stream=stream_name,
                reply_to_user_id=None,
            )

        # Always publish SkillResult(success=True) — the batch items are
        # side-effect publishes; the SkillResult is the primary contract.
        # Use the raw text as skill output for the caller.
        skill_result = SkillResult(
            invocation_id=message.invocation_id,
            skill_name=message.skill_name,
            success=True,
            output=result.text,
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
            skill_name=message.skill_name,
            success=False,
            error_message="Claude API call failed",
        )
        await _publish_response(
            connection,
            stream_name,
            skill_result,
            f"type=SkillResult invocation_id={message.invocation_id} success=False",
        )
