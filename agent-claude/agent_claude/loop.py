"""Message dispatch and processing loop for agent-claude.

Subscribes to all declared streams and dispatches incoming messages
to the Claude API based on type (ADR-020 dispatch table).

Dispatch table:
  TextMessage           -> user turn prompt; call Claude API
  CronEvent             -> structured cron prompt; call Claude API
  SkillInvocation (us)  -> skill prompt + SkillProgress; call Claude API
  SkillInvocation (other) -> drop silently
  SkillProgress         -> forward as TextMessage (no LLM call, FR78)
  TopologyShutdown      -> graceful drain
  Any other type        -> log debug, skip
"""

from __future__ import annotations

import asyncio
import json
import logging
from typing import Any

from wheelhouse.skills.library_ingest import (
    SKILL_REGISTRY as LIBRARY_SKILL_REGISTRY,
    run_library_ingest,
)
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
            config=config,
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
    config: dict[str, Any] | None = None,
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
                    config=config,
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
                    config=config,
                )
            elif isinstance(message, SkillProgress):
                await _handle_skill_progress(
                    message, connection, stream_name, agent_name,
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
    config: dict[str, Any] | None = None,
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

    # Attachment branch: if the surface forwarded a file attachment
    # alongside the text (e.g. a PDF dropped in Telegram), route the
    # bytes into the Library ingest skill instead of the Claude chat
    # loop. Gated on library_sandbox availability — if Library is
    # disabled or the sandbox failed to build, we fall through to the
    # normal chat path so the user still gets a useful response (the
    # bytes are dropped in that case; this is the intended degradation
    # per NFR24).
    if message.attachment_bytes:
        sandbox = (config or {}).get("library_sandbox")
        if sandbox is not None:
            await _handle_attached_ingest(
                message,
                connection,
                stream_name,
                claude_client,
                persona,
                agent_name,
                persona_path,
                config or {},
            )
            return
        logger.info(
            "TextMessage has attachment but Library sandbox is unavailable — "
            "ignoring attachment bytes and falling through to chat path: "
            "stream=%s filename=%s",
            stream_name,
            message.attachment_filename or "<unnamed>",
        )

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
            # Graceful fallback: Claude ignored the JSON format instruction.
            # Publish the raw text to the source stream so the user gets a response.
            logger.warning(
                "Malformed batch response from Claude — falling back to plain text: stream=%s",
                stream_name,
            )
            fallback = TextMessage(
                content=result.text,
                publisher_id=agent_name,
                reply_to_user_id=message.user_id,
            )
            await _publish_response(
                connection,
                stream_name,
                fallback,
                f"type=TextMessage(fallback) stream={stream_name} chars={len(result.text)}",
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


async def _handle_attached_ingest(
    message: TextMessage,
    connection: Any,
    stream_name: str,
    claude_client: ClaudeClient,
    persona: Persona,
    agent_name: str,
    persona_path: str,
    config: dict[str, Any],
) -> None:
    """Route a TextMessage with attachment bytes to the Library ingest skill.

    Called by ``_handle_text_message`` when ``message.attachment_bytes``
    is non-empty and a per-agent LibrarySandbox is available. Builds an
    inline ``library_ingest`` parameter map (using ``source_content``
    rather than ``source_ref`` so the skill does not need to re-read the
    bytes from inside the sandbox root) and dispatches the handler in a
    thread so the synchronous Claude summarizer does not block the event
    loop.

    After a successful ingest, the newly written pages are read back
    from the sandbox and injected into Claude's conversation session as
    a synthetic user turn — this is the ONLY way Claude's session can
    learn about the upload, because the attachment path bypasses the
    normal chat flow entirely. Without this priming, follow-up questions
    like "what do you know about this paper?" have no referent because
    Claude's session never saw the filename or the content.
    """
    sandbox = config.get("library_sandbox")
    library_status = config.get("library_status", "disabled")

    filename = message.attachment_filename or "attachment"
    mime_type = message.attachment_mime_type or ""
    source_type = _mime_to_source_type(mime_type, filename)

    # Publish a SkillProgress-style ack early so the user gets feedback
    # while Claude summarises (can take 10-30s).
    ack_text = TextMessage(
        content=f"Ingesting **{filename}** into the Library — one moment…",
        publisher_id=agent_name,
        reply_to_user_id=message.user_id,
    )
    await _publish_response(
        connection,
        stream_name,
        ack_text,
        f"type=TextMessage(ingest-ack) stream={stream_name} filename={filename}",
    )

    # PDFs round-trip via Latin-1 so the bytes survive the proto3
    # map<string,string> skill parameter; text/markdown is decoded as
    # UTF-8 with a clean reject on invalid sequences.
    try:
        source_content = _bytes_to_param(message.attachment_bytes, source_type)
    except UnicodeDecodeError:
        reply = TextMessage(
            content=(
                f"⚠ Could not ingest **{filename}**: the file is not valid "
                f"UTF-8 and `{source_type}` requires UTF-8. If it's a PDF, "
                f"check the filename extension."
            ),
            publisher_id=agent_name,
            reply_to_user_id=message.user_id,
        )
        await _publish_response(
            connection, stream_name, reply, "type=TextMessage(ingest-err)"
        )
        return

    parameters = {
        "source_type": source_type,
        "source_ref": filename,
        "source_content": source_content,
    }

    invocation_id = f"surface-{stream_name}-{message.timestamp_ms}"
    logger.info(
        "library_ingest triggered by surface attachment: stream=%s filename=%s "
        "size=%d source_type=%s",
        stream_name,
        filename,
        len(message.attachment_bytes),
        source_type,
    )

    skill_result: SkillResult = await asyncio.to_thread(
        run_library_ingest,
        sandbox,
        parameters,
        library_status=library_status,
        invocation_id=invocation_id,
    )

    if not skill_result.success:
        code = skill_result.error_code or "INGEST_FAILED"
        msg = skill_result.error_message or "unknown error"
        body = f"⚠ Could not ingest **{filename}** — `{code}`: {msg}"
        logger.warning(
            "library_ingest failed via surface attachment: "
            "filename=%s code=%s msg=%s",
            filename,
            code,
            msg,
        )
        reply = TextMessage(
            content=body,
            publisher_id=agent_name,
            reply_to_user_id=message.user_id,
        )
        await _publish_response(
            connection,
            stream_name,
            reply,
            f"type=TextMessage(ingest-err) stream={stream_name} "
            f"filename={filename} code={code}",
        )
        return

    # Success path: read the newly-written pages back from the sandbox
    # and prime Claude's conversation session with their content, so
    # the user's follow-up questions ("what do you know about this
    # paper?") have something to land on. Without this, Claude's
    # session has zero record of the upload because the attachment
    # bypassed the normal chat path entirely.
    page_count = skill_result.library_page_count or 0
    tokens = skill_result.library_tokens or 0
    logger.info(
        "library_ingest succeeded: filename=%s pages=%d tokens=%d — "
        "priming claude session",
        filename,
        page_count,
        tokens,
    )

    new_pages = _load_ingest_pages(sandbox, filename)
    synthetic_prompt = _build_ingest_session_prompt(
        filename, page_count, tokens, new_pages
    )

    # Re-read MEMORY.md (same as normal chat path) so the system prompt
    # is current.
    persona.reload_memory(persona_path)
    system_prompt = persona.build_system_prompt()

    result = await claude_client.complete(
        system_prompt=system_prompt,
        user_message=synthetic_prompt,
        msg_type="TextMessage(ingest-prime)",
        stream_name=stream_name,
        conversation_id=message.user_id or stream_name,
        timeout=180.0,  # Summarizer + confirmation together can take a while.
    )

    if result is None or not result.text.strip():
        # Fallback: Claude timed out or refused. Still tell the user the
        # ingest itself succeeded so they know the upload landed.
        fallback = TextMessage(
            content=(
                f"✓ Ingested **{filename}** into your Library "
                f"({page_count} page{'s' if page_count != 1 else ''}, "
                f"{tokens} tokens). Ask me about it anytime."
            ),
            publisher_id=agent_name,
            reply_to_user_id=message.user_id,
        )
        await _publish_response(
            connection,
            stream_name,
            fallback,
            f"type=TextMessage(ingest-result-fallback) stream={stream_name} "
            f"filename={filename}",
        )
        return

    # Parse the batch response like the normal chat path does so the
    # output routing respects ADR-022.
    items = parse_batch_response(result.text)
    if items is None:
        logger.warning(
            "Malformed batch response from ingest prime — falling back "
            "to plain text: stream=%s",
            stream_name,
        )
        fallback = TextMessage(
            content=result.text,
            publisher_id=agent_name,
            reply_to_user_id=message.user_id,
        )
        await _publish_response(
            connection,
            stream_name,
            fallback,
            f"type=TextMessage(ingest-prime-fallback) stream={stream_name} "
            f"filename={filename} chars={len(result.text)}",
        )
        return
    if not items:
        logger.debug("Empty ingest-prime batch response: stream=%s", stream_name)
        return
    await publish_batch(
        connection,
        items,
        agent_name,
        source_stream=stream_name,
        reply_to_user_id=message.user_id,
    )


def _load_ingest_pages(
    sandbox: Any, filename: str
) -> list[tuple[str, str]]:
    """Read pages just written by a library_ingest call for ``filename``.

    Uses ``.provenance.json`` (story 13-13) to enumerate the slugs that
    match the source name, then reads each page via the sandbox. Returns
    a list of ``(slug, body)`` tuples in provenance order, or an empty
    list on any failure — the caller's fallback path handles that.

    Kept defensive: provenance read errors, missing files, and invalid
    JSON are all swallowed so a corrupted sidecar can never block the
    chat reply.
    """
    try:
        provenance_raw = sandbox.read(".provenance.json")
    except Exception as exc:
        logger.debug("could not read .provenance.json: %s", exc)
        return []
    try:
        provenance = json.loads(provenance_raw)
    except Exception as exc:
        logger.debug("could not parse .provenance.json: %s", exc)
        return []

    entries = provenance.get("entries", {}) if isinstance(provenance, dict) else {}
    if not isinstance(entries, dict):
        return []

    matching_slugs: list[str] = []
    for slug, record in entries.items():
        if not isinstance(record, dict):
            continue
        src = record.get("source") or ""
        if src == filename:
            matching_slugs.append(slug)

    pages: list[tuple[str, str]] = []
    for slug in matching_slugs:
        try:
            body = sandbox.read(slug)
        except Exception as exc:
            logger.debug("could not read page %s: %s", slug, exc)
            continue
        pages.append((slug, body))
    return pages


def _build_ingest_session_prompt(
    filename: str,
    page_count: int,
    tokens: int,
    pages: list[tuple[str, str]],
) -> str:
    """Build the synthetic user-turn text that primes Claude's session
    with the content of a newly-ingested document.

    Rendered as a single long user message so Claude's ``claude -p``
    session (started or resumed under the user's ``conversation_id``)
    picks it up as real conversation history. Subsequent follow-up
    questions like "what do you know about this paper?" then resolve
    against this content without requiring Claude to go hunt the
    filesystem via its Bash tool.
    """
    parts: list[str] = []
    parts.append(
        f"I just uploaded a file via Telegram: **{filename}**. "
        f"You summarized it into your Library as "
        f"{page_count} page{'s' if page_count != 1 else ''} "
        f"({tokens} tokens total). The pages are shown below — treat "
        f"them as your working memory for this document. When I ask "
        f"follow-up questions about it, answer from these pages first, "
        f"and cite both the Library page slug and the source filename."
    )
    if pages:
        parts.append("---\n## Library pages just written\n")
        for slug, body in pages:
            parts.append(f"### `{slug}`\n\n{body.strip()}\n")
    else:
        parts.append(
            "_(I was unable to read the pages back from the sandbox — "
            "if the user asks a follow-up, grep `/workspace/.library/` "
            "directly.)_"
        )
    parts.append(
        "Acknowledge that you've read the document and give me a "
        "one-sentence summary. I'll ask follow-ups next."
    )
    return "\n\n".join(parts)


def _mime_to_source_type(mime: str, filename: str) -> str:
    """Map a MIME type (with filename fallback) to a library_ingest source_type.

    The 13-7 enum is `text | markdown | pdf | url`. Unknown types fall
    back to `text`, letting the skill's content-level validation produce
    a clear error — a fallback is better than a hard-reject at this
    layer because we've already paid for the download and the user
    deserves a specific error from the ingest pipeline rather than a
    vague surface rejection.
    """
    mime_lower = mime.lower().strip()
    if mime_lower == "application/pdf":
        return "pdf"
    if mime_lower in ("text/markdown", "text/x-markdown", "text/x-web-markdown"):
        return "markdown"
    if mime_lower.startswith("text/"):
        return "text"
    # Fallback on file extension.
    ext = filename.rsplit(".", 1)[-1].lower() if "." in filename else ""
    if ext == "pdf":
        return "pdf"
    if ext in ("md", "markdown"):
        return "markdown"
    return "text"


def _bytes_to_param(raw: bytes, source_type: str) -> str:
    """Encode attachment bytes for transit through the ingest skill's
    ``source_content`` parameter (which is typed ``str``).

    * PDF → Latin-1 round-trip: every byte maps to one codepoint in
      0..=255. The Python ingest skill's ``_resolve_pdf_bytes`` accepts
      a str whose first 5 characters are ``%PDF-`` and re-encodes via
      ``latin-1`` to recover the original bytes.
    * Text / markdown → strict UTF-8 decode. ``UnicodeDecodeError``
      propagates; the caller maps it to a user-facing error.
    """
    if source_type == "pdf":
        return raw.decode("latin-1")
    return raw.decode("utf-8")


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
            logger.warning(
                "Malformed batch response from Claude — falling back to plain text: stream=%s type=CronEvent",
                stream_name,
            )
            fallback = TextMessage(
                content=result.text,
                publisher_id=agent_name,
            )
            await _publish_response(
                connection,
                stream_name,
                fallback,
                f"type=TextMessage(fallback) stream={stream_name} chars={len(result.text)}",
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
    *,
    config: dict[str, Any] | None = None,
) -> None:
    """Handle an incoming SkillInvocation (AC #4, #7).

    Drops invocations addressed to other agents.
    Publishes SkillProgress within 2s before Claude API call (AC-06).

    Story 13-7: if ``message.skill_name`` is registered in the Library
    skill registry, dispatch to the Library handler instead of the
    generic Claude path. This bypasses LLM invocation entirely for
    ``library_ingest`` (and later 13-14 retrieval, 13-16 lint).
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

    # Story 13-7: Library skill dispatch interception. Happens AFTER the
    # agent_id drop (Library skills still honour targeting) but BEFORE
    # any Claude call. Non-library skills (skill_name not in the
    # registry) fall through to the generic Claude path unchanged.
    if message.skill_name in LIBRARY_SKILL_REGISTRY:
        await _handle_library_skill_invocation(
            message,
            connection,
            stream_name,
            config=config,
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


async def _handle_library_skill_invocation(
    message: SkillInvocation,
    connection: Any,
    stream_name: str,
    *,
    config: dict[str, Any] | None = None,
) -> None:
    """Dispatch a Library skill invocation to its registered handler (Story 13-7).

    The generic Claude path is bypassed entirely — the Library handler
    constructs the ``SkillResult`` directly from the pre-built
    ``LibrarySandbox`` and the validated parameters. Preserves the
    5-4 / ADR-035 SkillProgress ack contract by publishing a
    ``"Processing..."`` progress message before calling the handler.
    """
    # Publish SkillProgress immediately (preserves the 2-second ack
    # contract from 5-4 / ADR-035 — surfaces display "processing" while
    # the handler runs).
    progress = SkillProgress(
        invocation_id=message.invocation_id,
        skill_name=message.skill_name,
        status_message="Processing...",
    )
    await connection.publish(stream_name, progress)
    logger.debug(
        "SkillProgress published (library): invocation_id=%s stream=%s skill=%s",
        message.invocation_id,
        stream_name,
        message.skill_name,
    )

    # Resolve the handler from the registry. ``LIBRARY_SKILL_REGISTRY``
    # is module-level; the dispatch caller has already confirmed the
    # skill_name is registered, so this lookup is infallible.
    handler = LIBRARY_SKILL_REGISTRY[message.skill_name]

    # Degrade safely when the startup path did not attach a config dict
    # — treat as disabled so the handler produces a clean
    # LIBRARY_DISABLED refusal instead of crashing.
    cfg = config or {}
    sandbox = cfg.get("library_sandbox")
    library_status = cfg.get("library_status", "disabled")

    skill_result: SkillResult = handler(
        sandbox,
        dict(message.parameters) if message.parameters else {},
        library_status=library_status,
        invocation_id=message.invocation_id,
        skill_name=message.skill_name,
    )

    await _publish_response(
        connection,
        stream_name,
        skill_result,
        f"type=SkillResult invocation_id={message.invocation_id} "
        f"skill={message.skill_name} success={skill_result.success} "
        f"error_code={skill_result.error_code or '<none>'}",
    )


async def _handle_skill_progress(
    message: SkillProgress,
    connection: Any,
    stream_name: str,
    agent_name: str,
) -> None:
    """Handle an incoming SkillProgress by forwarding as TextMessage (FR78).

    SkillProgress messages are published by the broker per output line during
    skill execution (E12-21). The agent forwards them as TextMessages so
    surfaces (Telegram, CLI) display real-time progress.

    No Claude API call is made — this is a direct pass-through.
    """
    # Skip empty progress updates (AC-5)
    if not message.status_message:
        logger.debug(
            "SkillProgress skipped (empty status_message): invocation_id=%s stream=%s",
            message.invocation_id,
            stream_name,
        )
        return

    progress_text = TextMessage(
        content=message.status_message,
        publisher_id=agent_name,
    )
    await _publish_response(
        connection,
        stream_name,
        progress_text,
        f"type=TextMessage(progress) invocation_id={message.invocation_id} stream={stream_name} chars={len(message.status_message)}",
    )
