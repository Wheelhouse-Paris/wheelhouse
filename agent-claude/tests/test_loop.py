"""Acceptance tests for Stories 8.2 and 8.3: Message Processing Loop and Response Publishing.

Tests verify all acceptance criteria for the message dispatch loop and response publishing.

Story 8.2 Acceptance Criteria:
  AC1: Agent subscribes to every stream in WH_STREAMS; info log lists them
  AC2: TextMessage content passed to Claude API with persona as system prompt
  AC3: CronEvent rendered as structured prompt and passed to Claude API
  AC4: SkillInvocation (for us) -> SkillProgress published + Claude API call
  AC5: Claude API timeout (60s) -> loop continues, error logged
  AC6: Self-echo filter drops TextMessage where publisher == agent_name
  AC7: SkillInvocation for other agent dropped silently
  AC8: MEMORY.md re-read before each Claude API call

Story 8.3 Acceptance Criteria:
  AC1: TextMessage response published to same stream with publisher=WH_AGENT_NAME
  AC2: Response appears with agent name as publisher
  AC3: CronEvent response published as TextMessage
  AC4: SkillInvocation -> SkillResult(success=True) with invocation_id
  AC5: API timeout -> no publish for TextMessage/CronEvent
  AC6: SkillInvocation failure -> SkillResult(success=False) published
  AC7: Publish failure -> warning logged, no crash
"""

from __future__ import annotations

import asyncio
import logging
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from wheelhouse.types import (
    CronEvent,
    SkillInvocation,
    SkillProgress,
    SkillResult,
    TextMessage,
    TopologyShutdown,
)


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(autouse=True)
def _reset_loop_state():
    """Reset module-level state in loop.py between tests."""
    from agent_claude import loop
    loop._pending_tasks.clear()
    loop._shutdown_event = asyncio.Event()
    loop._fatal_error = None
    yield
    loop._pending_tasks.clear()
    loop._shutdown_event = asyncio.Event()
    loop._fatal_error = None


@pytest.fixture
def mock_connection():
    """Create a mock SDK connection."""
    conn = AsyncMock()
    conn.publish = AsyncMock()
    conn.subscribe = AsyncMock()
    conn.close = AsyncMock()
    return conn


@pytest.fixture
def mock_claude_client():
    """Create a mock ClaudeClient."""
    from agent_claude.claude_client import CompletionResult

    client = AsyncMock()
    client.complete = AsyncMock(return_value=CompletionResult(
        text="Hello from Claude",
        input_tokens=100,
        output_tokens=50,
    ))
    return client


@pytest.fixture
def persona(tmp_path):
    """Create a test Persona with real files."""
    from agent_claude.persona import Persona

    soul_file = tmp_path / "SOUL.md"
    soul_file.write_text("You are a helpful agent.", encoding="utf-8")
    identity_file = tmp_path / "IDENTITY.md"
    identity_file.write_text("Name: TestAgent", encoding="utf-8")
    memory_file = tmp_path / "MEMORY.md"
    memory_file.write_text("Remember: test context.", encoding="utf-8")

    return Persona(
        soul="You are a helpful agent.",
        identity="Name: TestAgent",
        memory="Remember: test context.",
    )


@pytest.fixture
def config(tmp_path):
    """Create test configuration."""
    return {
        "api_key": "test-api-key",
        "wh_url": "tcp://127.0.0.1:5555",
        "agent_name": "donna",
        "streams": ["main", "events"],
        "persona_path": str(tmp_path),
        "model": "claude-3-5-sonnet-20241022",
    }


# ---------------------------------------------------------------------------
# AC1: Subscribe to all streams in WH_STREAMS
# ---------------------------------------------------------------------------

class TestStreamSubscription:
    """Agent subscribes to every stream listed in WH_STREAMS."""

    async def test_subscribes_to_all_streams(
        self, mock_connection, config, persona, mock_claude_client
    ):
        """Given WH_STREAMS=main,events,
        When the message loop starts,
        Then connection.subscribe() is called for each stream.
        """
        from agent_claude.loop import run_message_loop
        from agent_claude import loop

        # Schedule shutdown after loop starts (clear() runs first in run_message_loop)
        async def _set_shutdown():
            await asyncio.sleep(0)
            loop._shutdown_event.set()

        asyncio.get_event_loop().create_task(_set_shutdown())
        await run_message_loop(mock_connection, config, persona, mock_claude_client)

        assert mock_connection.subscribe.call_count == 2
        stream_names = [call.args[0] for call in mock_connection.subscribe.call_args_list]
        assert "main" in stream_names
        assert "events" in stream_names

    async def test_logs_subscribed_streams(
        self, mock_connection, config, persona, mock_claude_client, caplog
    ):
        """Given WH_STREAMS=main,events,
        When subscriptions complete,
        Then an info log lists the streams.
        """
        from agent_claude.loop import run_message_loop
        from agent_claude import loop

        async def _set_shutdown():
            await asyncio.sleep(0)
            loop._shutdown_event.set()

        asyncio.get_event_loop().create_task(_set_shutdown())
        with caplog.at_level(logging.INFO, logger="agent_claude"):
            await run_message_loop(mock_connection, config, persona, mock_claude_client)

        assert any(
            "subscribed to streams: [main, events]" in record.message
            for record in caplog.records
        )


# ---------------------------------------------------------------------------
# AC2: TextMessage dispatched to Claude API with persona as system prompt
# ---------------------------------------------------------------------------

class TestTextMessageDispatch:
    """TextMessage content passed to Claude API with persona as system prompt."""

    async def test_text_message_calls_claude_api(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given a TextMessage arrives on a subscribed stream,
        When the message handler fires,
        Then the content is passed to Claude API with persona as system prompt.
        """
        from agent_claude.loop import _make_handler

        # Create MEMORY.md for reload
        (tmp_path / "MEMORY.md").write_text("Remember: test context.", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        msg = TextMessage(content="Hello agent", publisher="user1")
        await handler(msg)

        mock_claude_client.complete.assert_called_once()
        call_kwargs = mock_claude_client.complete.call_args
        assert "Hello agent" in call_kwargs.kwargs.get("user_message", call_kwargs.args[1] if len(call_kwargs.args) > 1 else "")
        # System prompt should contain persona content
        system_prompt = call_kwargs.kwargs.get("system_prompt", call_kwargs.args[0] if call_kwargs.args else "")
        assert "You are a helpful agent." in system_prompt

    async def test_text_message_logs_at_debug(
        self, mock_connection, config, persona, mock_claude_client, tmp_path, caplog
    ):
        """Claude API call is logged at debug level with message type and stream name."""
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        with caplog.at_level(logging.DEBUG, logger="agent_claude"):
            await handler(TextMessage(content="test", publisher="user1"))

        assert any("TextMessage" in r.message and "main" in r.message for r in caplog.records)


# ---------------------------------------------------------------------------
# AC3: CronEvent rendered as structured prompt
# ---------------------------------------------------------------------------

class TestCronEventDispatch:
    """CronEvent rendered as structured prompt and passed to Claude API."""

    async def test_cron_event_uses_template(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given a CronEvent arrives,
        When the handler fires,
        Then the cron prompt template is used.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="events",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        msg = CronEvent(
            job_name="daily-report",
            triggered_at="2026-03-13T10:00:00Z",
            publisher="system",
        )
        await handler(msg)

        mock_claude_client.complete.assert_called_once()
        call_kwargs = mock_claude_client.complete.call_args
        user_message = call_kwargs.kwargs.get("user_message", "")
        assert "daily-report" in user_message
        assert "2026-03-13T10:00:00Z" in user_message
        assert "Review your current context" in user_message


# ---------------------------------------------------------------------------
# AC4: SkillInvocation -> SkillProgress + Claude API call
# ---------------------------------------------------------------------------

class TestSkillInvocationDispatch:
    """SkillInvocation (for this agent) -> SkillProgress published + Claude API."""

    async def test_skill_invocation_publishes_progress(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given a SkillInvocation arrives for this agent,
        When the handler fires,
        Then SkillProgress is published before the Claude API call.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        msg = SkillInvocation(
            invocation_id="inv-001",
            skill_name="summarize",
            input_payload="Summarize this document.",
            target_agent="donna",
            publisher="operator",
        )
        await handler(msg)

        # SkillProgress should have been published first, then SkillResult
        assert mock_connection.publish.call_count == 2
        first_pub = mock_connection.publish.call_args_list[0]
        assert first_pub.args[0] == "main"
        progress = first_pub.args[1]
        assert isinstance(progress, SkillProgress)
        assert progress.invocation_id == "inv-001"
        assert progress.status == "IN_PROGRESS"
        assert progress.publisher == "donna"

        # Claude API should have been called with skill prompt
        mock_claude_client.complete.assert_called_once()
        call_kwargs = mock_claude_client.complete.call_args
        user_message = call_kwargs.kwargs.get("user_message", "")
        assert "summarize" in user_message
        assert "Summarize this document." in user_message

    async def test_skill_invocation_uses_prompt_template(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """SkillInvocation prompt matches ADR-017 template."""
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        msg = SkillInvocation(
            invocation_id="inv-002",
            skill_name="translate",
            input_payload="Translate to French.",
            target_agent="donna",
        )
        await handler(msg)

        call_kwargs = mock_claude_client.complete.call_args
        user_message = call_kwargs.kwargs.get("user_message", "")
        assert "Skill 'translate' has been invoked" in user_message
        assert "Translate to French." in user_message
        assert "describe what you will do" in user_message


# ---------------------------------------------------------------------------
# AC5: Claude API timeout -> loop continues
# ---------------------------------------------------------------------------

class TestClaudeApiTimeout:
    """Claude API timeout (60s) -> loop continues, error logged."""

    async def test_timeout_returns_none(self):
        """Given ClaudeClient.complete() times out,
        When the timeout fires,
        Then it returns None.
        """
        from agent_claude.claude_client import ClaudeClient

        client = ClaudeClient(api_key="test-key")

        # Mock the internal client to simulate timeout
        async def slow_call(*args, **kwargs):
            await asyncio.sleep(10)

        with patch.object(client, '_client') as mock_client:
            mock_client.messages.create.side_effect = lambda **kw: asyncio.sleep(10)

            # Use a very short timeout
            with patch('agent_claude.claude_client.asyncio.wait_for', side_effect=asyncio.TimeoutError):
                result = await client.complete(
                    system_prompt="test",
                    user_message="test",
                    timeout=0.01,
                    msg_type="TextMessage",
                    stream_name="main",
                )

            assert result is None

    async def test_timeout_logs_error(self, caplog):
        """Timeout is logged at error level with type, stream, elapsed time."""
        from agent_claude.claude_client import ClaudeClient

        client = ClaudeClient(api_key="test-key")

        with patch('agent_claude.claude_client.asyncio.wait_for', side_effect=asyncio.TimeoutError):
            with caplog.at_level(logging.ERROR, logger="agent_claude"):
                await client.complete(
                    system_prompt="test",
                    user_message="test",
                    timeout=0.01,
                    msg_type="TextMessage",
                    stream_name="main",
                )

        assert any(
            "timed out" in r.message and "TextMessage" in r.message and "main" in r.message
            for r in caplog.records
        )

    async def test_loop_continues_after_timeout(
        self, mock_connection, config, persona, tmp_path
    ):
        """After timeout, the handler returns and the loop continues."""
        from agent_claude.claude_client import CompletionResult
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        # First call times out, second succeeds
        mock_client = AsyncMock()
        mock_client.complete = AsyncMock(side_effect=[
            None,  # timeout returns None
            CompletionResult(text="ok", input_tokens=10, output_tokens=5),
        ])

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        # Both calls should complete without exception
        await handler(TextMessage(content="msg1", publisher="user1"))
        await handler(TextMessage(content="msg2", publisher="user1"))

        assert mock_client.complete.call_count == 2


# ---------------------------------------------------------------------------
# AC6: Self-echo filter
# ---------------------------------------------------------------------------

class TestSelfEchoFilter:
    """Self-echo filter drops TextMessage where publisher == agent_name."""

    async def test_self_echo_dropped(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given TextMessage.publisher == WH_AGENT_NAME,
        When the handler fires,
        Then no Claude API call is made.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        msg = TextMessage(content="My own response", publisher="donna")
        await handler(msg)

        mock_claude_client.complete.assert_not_called()

    async def test_self_echo_logged_at_debug(
        self, mock_connection, config, persona, mock_claude_client, tmp_path, caplog
    ):
        """Self-echo filter logs at debug level."""
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        with caplog.at_level(logging.DEBUG, logger="agent_claude"):
            await handler(TextMessage(content="echo", publisher="donna"))

        assert any("Self-message filtered" in r.message for r in caplog.records)

    async def test_other_publisher_not_filtered(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """TextMessage from a different publisher is NOT filtered."""
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        msg = TextMessage(content="Hello", publisher="alice")
        await handler(msg)

        mock_claude_client.complete.assert_called_once()


# ---------------------------------------------------------------------------
# AC7: SkillInvocation for other agent dropped
# ---------------------------------------------------------------------------

class TestSkillInvocationOtherAgent:
    """SkillInvocation addressed to different agent is dropped."""

    async def test_other_agent_invocation_dropped(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given SkillInvocation.target_agent != WH_AGENT_NAME,
        When the handler fires,
        Then no Claude API call or SkillProgress publish happens.
        """
        from agent_claude.loop import _make_handler

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        msg = SkillInvocation(
            invocation_id="inv-003",
            skill_name="summarize",
            input_payload="data",
            target_agent="bob",
            publisher="operator",
        )
        await handler(msg)

        mock_claude_client.complete.assert_not_called()
        mock_connection.publish.assert_not_called()

    async def test_other_agent_logged_at_debug(
        self, mock_connection, config, persona, mock_claude_client, tmp_path, caplog
    ):
        """Dropped SkillInvocation is logged at debug level."""
        from agent_claude.loop import _make_handler

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        with caplog.at_level(logging.DEBUG, logger="agent_claude"):
            await handler(SkillInvocation(
                target_agent="bob",
                skill_name="x",
                publisher="op",
            ))

        assert any("dropped" in r.message.lower() for r in caplog.records)

    async def test_target_agent_exact_match(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """target_agent comparison is exact, case-sensitive string match."""
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        # "Donna" (capitalized) should NOT match "donna"
        msg = SkillInvocation(
            target_agent="Donna",
            skill_name="x",
            input_payload="y",
            publisher="op",
        )
        await handler(msg)

        mock_claude_client.complete.assert_not_called()
        mock_connection.publish.assert_not_called()


# ---------------------------------------------------------------------------
# AC8: MEMORY.md re-read before each Claude API call
# ---------------------------------------------------------------------------

class TestMemoryReload:
    """MEMORY.md is re-read from disk before each Claude API call."""

    async def test_memory_reread_before_api_call(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given MEMORY.md changes between calls,
        When a TextMessage is processed,
        Then the updated memory is used in the system prompt.
        """
        from agent_claude.loop import _make_handler

        memory_file = tmp_path / "MEMORY.md"
        memory_file.write_text("Original memory.", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        # First call with original memory
        await handler(TextMessage(content="msg1", publisher="user1"))

        first_call = mock_claude_client.complete.call_args
        first_system = first_call.kwargs.get("system_prompt", "")
        assert "Original memory." in first_system

        # Update MEMORY.md
        memory_file.write_text("Updated memory!", encoding="utf-8")

        # Second call should pick up new memory
        await handler(TextMessage(content="msg2", publisher="user1"))

        second_call = mock_claude_client.complete.call_args
        second_system = second_call.kwargs.get("system_prompt", "")
        assert "Updated memory!" in second_system

    async def test_memory_reload_for_cron_event(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """MEMORY.md is also re-read for CronEvent processing."""
        from agent_claude.loop import _make_handler

        memory_file = tmp_path / "MEMORY.md"
        memory_file.write_text("Cron memory.", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="events",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(CronEvent(job_name="test", triggered_at="now"))

        call_kwargs = mock_claude_client.complete.call_args
        system_prompt = call_kwargs.kwargs.get("system_prompt", "")
        assert "Cron memory." in system_prompt


# ---------------------------------------------------------------------------
# Unknown message type
# ---------------------------------------------------------------------------

class TestUnknownMessageType:
    """Unknown message types logged at debug and skipped."""

    async def test_unknown_type_skipped(
        self, mock_connection, config, persona, mock_claude_client, tmp_path, caplog
    ):
        """Given an unknown message type arrives,
        When the handler fires,
        Then it is logged at debug and no Claude API call is made.
        """
        from agent_claude.loop import _make_handler

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        class UnknownMsg:
            pass

        with caplog.at_level(logging.DEBUG, logger="agent_claude"):
            await handler(UnknownMsg())

        mock_claude_client.complete.assert_not_called()
        assert any("Unknown type skipped" in r.message for r in caplog.records)


# ---------------------------------------------------------------------------
# TopologyShutdown triggers graceful drain
# ---------------------------------------------------------------------------

class TestTopologyShutdown:
    """TopologyShutdown triggers graceful drain."""

    async def test_topology_shutdown_sets_event(
        self, mock_connection, config, persona, mock_claude_client, tmp_path, caplog
    ):
        """Given a TopologyShutdown arrives,
        When the handler fires,
        Then the shutdown event is set.
        """
        from agent_claude.loop import _make_handler, _shutdown_event

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        assert not _shutdown_event.is_set()

        with caplog.at_level(logging.INFO, logger="agent_claude"):
            await handler(TopologyShutdown(reason="operator request"))

        assert _shutdown_event.is_set()
        assert any("TopologyShutdown" in r.message for r in caplog.records)


# ---------------------------------------------------------------------------
# ClaudeClient unit tests
# ---------------------------------------------------------------------------

class TestClaudeClient:
    """Unit tests for ClaudeClient."""

    async def test_complete_calls_anthropic_correctly(self):
        """ClaudeClient.complete() calls anthropic.Anthropic.messages.create
        with correct parameters.
        """
        from agent_claude.claude_client import ClaudeClient

        client = ClaudeClient(api_key="test-key", model="claude-test-model")

        mock_response = MagicMock()
        mock_response.content = [MagicMock(text="Test response")]
        mock_response.usage = MagicMock(input_tokens=50, output_tokens=25)

        with patch.object(client._client.messages, 'create', return_value=mock_response):
            result = await client.complete(
                system_prompt="You are helpful.",
                user_message="Hello!",
                msg_type="TextMessage",
                stream_name="main",
            )

        assert result is not None
        assert result.text == "Test response"
        assert result.input_tokens == 50
        assert result.output_tokens == 25

    async def test_auth_error_raises_claude_auth_error(self):
        """anthropic.AuthenticationError -> ClaudeAuthError."""
        import anthropic
        from agent_claude.claude_client import ClaudeClient
        from agent_claude.errors import ClaudeAuthError

        client = ClaudeClient(api_key="bad-key")

        mock_response = MagicMock()
        mock_response.status_code = 401
        auth_err = anthropic.AuthenticationError(
            message="Invalid API key",
            response=mock_response,
            body={"error": {"message": "Invalid API key"}},
        )

        with patch.object(client._client.messages, 'create', side_effect=auth_err):
            with pytest.raises(ClaudeAuthError, match="authentication failed"):
                await client.complete(
                    system_prompt="test",
                    user_message="test",
                )

    async def test_transient_api_error_returns_none(self, caplog):
        """Transient API errors return None and log at warn level."""
        import anthropic
        from agent_claude.claude_client import ClaudeClient

        client = ClaudeClient(api_key="test-key")

        mock_response = MagicMock()
        mock_response.status_code = 500
        api_err = anthropic.APIError(
            message="Internal server error",
            request=MagicMock(),
            body={"error": {"message": "Internal server error"}},
        )

        with patch.object(client._client.messages, 'create', side_effect=api_err):
            with caplog.at_level(logging.WARNING, logger="agent_claude"):
                result = await client.complete(
                    system_prompt="test",
                    user_message="test",
                    msg_type="TextMessage",
                    stream_name="main",
                )

        assert result is None
        assert any("transient error" in r.message.lower() for r in caplog.records)


# ---------------------------------------------------------------------------
# Persona reload_memory tests
# ---------------------------------------------------------------------------

class TestPersonaReloadMemory:
    """Persona.reload_memory() re-reads MEMORY.md from disk."""

    def test_reload_memory_updates_content(self, tmp_path):
        """reload_memory() picks up changes to MEMORY.md."""
        from agent_claude.persona import Persona

        memory_file = tmp_path / "MEMORY.md"
        memory_file.write_text("initial", encoding="utf-8")

        p = Persona(soul="s", identity="i", memory="initial")
        assert p.memory == "initial"

        memory_file.write_text("updated", encoding="utf-8")
        p.reload_memory(str(tmp_path))

        assert p.memory == "updated"
        assert "updated" in p.build_system_prompt()

    def test_reload_memory_missing_file(self, tmp_path):
        """reload_memory() sets memory to empty when file is absent."""
        from agent_claude.persona import Persona

        p = Persona(soul="s", identity="i", memory="original")
        p.reload_memory(str(tmp_path))

        assert p.memory == ""


# ---------------------------------------------------------------------------
# Type serialization round-trip tests
# ---------------------------------------------------------------------------

class TestTypeStubs:
    """Verify new type stubs serialize/deserialize correctly."""

    def test_text_message_publisher_field(self):
        """TextMessage now has publisher field."""
        msg = TextMessage(content="hello", publisher="donna")
        data = msg.SerializeToString()
        restored = TextMessage.FromString(data)
        assert restored.publisher == "donna"
        assert restored.content == "hello"

    def test_cron_event_round_trip(self):
        """CronEvent serializes and deserializes correctly."""
        msg = CronEvent(job_name="daily", triggered_at="2026-01-01T00:00:00Z", publisher="system")
        data = msg.SerializeToString()
        restored = CronEvent.FromString(data)
        assert restored.job_name == "daily"
        assert restored.triggered_at == "2026-01-01T00:00:00Z"

    def test_skill_invocation_round_trip(self):
        """SkillInvocation serializes and deserializes correctly."""
        msg = SkillInvocation(
            invocation_id="inv-1",
            skill_name="summarize",
            input_payload="data",
            target_agent="donna",
            publisher="op",
        )
        data = msg.SerializeToString()
        restored = SkillInvocation.FromString(data)
        assert restored.invocation_id == "inv-1"
        assert restored.target_agent == "donna"

    def test_skill_progress_round_trip(self):
        """SkillProgress serializes and deserializes correctly."""
        msg = SkillProgress(invocation_id="inv-1", status="IN_PROGRESS", message="Working...")
        data = msg.SerializeToString()
        restored = SkillProgress.FromString(data)
        assert restored.status == "IN_PROGRESS"

    def test_skill_result_round_trip(self):
        """SkillResult serializes and deserializes correctly."""
        from wheelhouse.types import SkillResult
        msg = SkillResult(invocation_id="inv-1", success=True, payload="output")
        data = msg.SerializeToString()
        restored = SkillResult.FromString(data)
        assert restored.success is True
        assert restored.payload == "output"

    def test_topology_shutdown_round_trip(self):
        """TopologyShutdown serializes and deserializes correctly."""
        msg = TopologyShutdown(reason="operator request")
        data = msg.SerializeToString()
        restored = TopologyShutdown.FromString(data)
        assert restored.reason == "operator request"


# ===========================================================================
# Story 8.3: Publish Claude API Responses
# ===========================================================================


# ---------------------------------------------------------------------------
# 8.3 AC1: TextMessage response published to same stream
# ---------------------------------------------------------------------------

class TestTextMessageResponsePublishing:
    """TextMessage response published to same stream with correct publisher."""

    async def test_text_message_response_published(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given Claude API returns a completion for TextMessage,
        When the response is received,
        Then a TextMessage is published to the same stream with publisher=agent_name.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(TextMessage(content="Hello", publisher="user1"))

        # connection.publish should have been called with the response
        mock_connection.publish.assert_called_once()
        pub_args = mock_connection.publish.call_args
        assert pub_args.args[0] == "main"  # same stream
        response = pub_args.args[1]
        assert isinstance(response, TextMessage)
        assert response.publisher == "donna"
        assert response.content == "Hello from Claude"

    async def test_text_message_response_content_matches_api(
        self, mock_connection, config, persona, tmp_path
    ):
        """Response TextMessage content matches Claude API result text."""
        from agent_claude.claude_client import CompletionResult
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        client = AsyncMock()
        client.complete = AsyncMock(return_value=CompletionResult(
            text="This is the API response text",
            input_tokens=50,
            output_tokens=30,
        ))

        handler = _make_handler(
            connection=mock_connection,
            stream_name="events",
            claude_client=client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(TextMessage(content="test", publisher="user1"))

        response = mock_connection.publish.call_args.args[1]
        assert response.content == "This is the API response text"


# ---------------------------------------------------------------------------
# 8.3 AC3: CronEvent response published as TextMessage
# ---------------------------------------------------------------------------

class TestCronEventResponsePublishing:
    """CronEvent response published as TextMessage."""

    async def test_cron_response_published_as_text_message(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given a CronEvent is processed and Claude API returns,
        When the response is received,
        Then a TextMessage is published with publisher=agent_name.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="events",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(CronEvent(
            job_name="daily-report",
            triggered_at="2026-03-13T10:00:00Z",
            publisher="system",
        ))

        mock_connection.publish.assert_called_once()
        pub_args = mock_connection.publish.call_args
        assert pub_args.args[0] == "events"  # same stream
        response = pub_args.args[1]
        assert isinstance(response, TextMessage)
        assert response.publisher == "donna"
        assert response.content == "Hello from Claude"


# ---------------------------------------------------------------------------
# 8.3 AC4: SkillInvocation -> SkillResult(success=True)
# ---------------------------------------------------------------------------

class TestSkillResultPublishing:
    """SkillInvocation -> SkillResult published with correct fields."""

    async def test_skill_success_publishes_skill_result(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Given a SkillInvocation is processed and Claude API succeeds,
        When the response is received,
        Then a SkillResult(success=True) is published with invocation_id.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(SkillInvocation(
            invocation_id="inv-100",
            skill_name="summarize",
            input_payload="Summarize this.",
            target_agent="donna",
            publisher="operator",
        ))

        # Should have published SkillProgress AND SkillResult
        assert mock_connection.publish.call_count == 2

        # First call: SkillProgress (from 8.2)
        first_pub = mock_connection.publish.call_args_list[0]
        assert isinstance(first_pub.args[1], SkillProgress)

        # Second call: SkillResult (from 8.3)
        second_pub = mock_connection.publish.call_args_list[1]
        assert second_pub.args[0] == "main"
        result = second_pub.args[1]
        assert isinstance(result, SkillResult)
        assert result.invocation_id == "inv-100"
        assert result.success is True
        assert result.payload == "Hello from Claude"
        assert result.publisher == "donna"

    async def test_skill_failure_publishes_skill_result_false(
        self, mock_connection, config, persona, tmp_path
    ):
        """Given a SkillInvocation is processed and Claude API fails (returns None),
        When the handler processes the result,
        Then a SkillResult(success=False) is published.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        client = AsyncMock()
        client.complete = AsyncMock(return_value=None)  # API failure

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(SkillInvocation(
            invocation_id="inv-200",
            skill_name="translate",
            input_payload="Translate this.",
            target_agent="donna",
            publisher="operator",
        ))

        # Should have published SkillProgress AND SkillResult(success=False)
        assert mock_connection.publish.call_count == 2

        second_pub = mock_connection.publish.call_args_list[1]
        result = second_pub.args[1]
        assert isinstance(result, SkillResult)
        assert result.invocation_id == "inv-200"
        assert result.success is False
        assert "failed" in result.payload.lower()
        assert result.publisher == "donna"


# ---------------------------------------------------------------------------
# 8.3 AC5: API timeout -> no publish for TextMessage/CronEvent
# ---------------------------------------------------------------------------

class TestNoPublishOnTimeout:
    """No message published when Claude API returns None for TextMessage/CronEvent."""

    async def test_text_message_timeout_no_publish(
        self, mock_connection, config, persona, tmp_path
    ):
        """Given Claude API times out for a TextMessage,
        When the handler processes the None result,
        Then no message is published.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        client = AsyncMock()
        client.complete = AsyncMock(return_value=None)

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(TextMessage(content="test", publisher="user1"))

        mock_connection.publish.assert_not_called()

    async def test_cron_event_timeout_no_publish(
        self, mock_connection, config, persona, tmp_path
    ):
        """Given Claude API times out for a CronEvent,
        When the handler processes the None result,
        Then no message is published.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        client = AsyncMock()
        client.complete = AsyncMock(return_value=None)

        handler = _make_handler(
            connection=mock_connection,
            stream_name="events",
            claude_client=client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        await handler(CronEvent(
            job_name="test-cron",
            triggered_at="2026-03-13T10:00:00Z",
        ))

        mock_connection.publish.assert_not_called()


# ---------------------------------------------------------------------------
# 8.3 AC7: Publish failure -> warning logged, no crash
# ---------------------------------------------------------------------------

class TestPublishFailureHandling:
    """Publish failure is caught, logged at warning, agent continues."""

    async def test_publish_failure_logged_warning(
        self, mock_connection, config, persona, mock_claude_client, tmp_path, caplog
    ):
        """Given connection.publish() raises an exception,
        When the handler tries to publish the response,
        Then a warning is logged and the handler completes without crash.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        # Make publish raise
        mock_connection.publish = AsyncMock(
            side_effect=RuntimeError("broker unreachable")
        )

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            # Should NOT raise
            await handler(TextMessage(content="test", publisher="user1"))

        assert any(
            "Failed to publish response" in r.message
            for r in caplog.records
        )

    async def test_publish_failure_does_not_crash_loop(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """After a publish failure, subsequent messages can still be processed."""
        from agent_claude.claude_client import CompletionResult
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        call_count = 0

        async def publish_side_effect(*args, **kwargs):
            nonlocal call_count
            call_count += 1
            if call_count == 1:
                raise RuntimeError("first publish fails")
            # Second publish succeeds

        mock_connection.publish = AsyncMock(side_effect=publish_side_effect)

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        # First message: publish fails but handler completes
        await handler(TextMessage(content="msg1", publisher="user1"))
        # Second message: publish succeeds
        await handler(TextMessage(content="msg2", publisher="user1"))

        assert mock_claude_client.complete.call_count == 2


# ---------------------------------------------------------------------------
# 8.3 Self-echo integration: published responses get filtered on re-receive
# ---------------------------------------------------------------------------

class TestSelfEchoIntegration:
    """Published responses use agent_name as publisher; self-echo filter catches them."""

    async def test_response_publisher_matches_agent_name(
        self, mock_connection, config, persona, mock_claude_client, tmp_path
    ):
        """Published TextMessage has publisher == agent_name,
        so if re-received it would be filtered by self-echo filter.
        """
        from agent_claude.loop import _make_handler

        (tmp_path / "MEMORY.md").write_text("", encoding="utf-8")

        handler = _make_handler(
            connection=mock_connection,
            stream_name="main",
            claude_client=mock_claude_client,
            persona=persona,
            agent_name="donna",
            persona_path=str(tmp_path),
        )

        # Process incoming message -> response published
        await handler(TextMessage(content="test", publisher="user1"))

        response = mock_connection.publish.call_args.args[1]
        assert response.publisher == "donna"

        # Now simulate receiving that response back (self-echo)
        mock_claude_client.complete.reset_mock()
        await handler(response)

        # Should be filtered -- no second API call
        mock_claude_client.complete.assert_not_called()


# ---------------------------------------------------------------------------
# 8.3 _publish_response helper unit test
# ---------------------------------------------------------------------------

class TestPublishResponseHelper:
    """Unit tests for _publish_response helper."""

    async def test_publish_response_success_logs_info(self, caplog):
        """Successful publish logs at info level."""
        from agent_claude.loop import _publish_response

        conn = AsyncMock()
        conn.publish = AsyncMock()
        msg = TextMessage(content="test", publisher="donna")

        with caplog.at_level(logging.INFO, logger="agent_claude"):
            await _publish_response(conn, "main", msg, "type=TextMessage chars=4")

        conn.publish.assert_called_once_with("main", msg)
        assert any("Response published" in r.message for r in caplog.records)

    async def test_publish_response_failure_logs_warning(self, caplog):
        """Failed publish logs at warning level with exc_info."""
        from agent_claude.loop import _publish_response

        conn = AsyncMock()
        conn.publish = AsyncMock(side_effect=RuntimeError("connection lost"))
        msg = TextMessage(content="test", publisher="donna")

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            await _publish_response(conn, "main", msg, "type=TextMessage chars=4")

        assert any("Failed to publish" in r.message for r in caplog.records)


# ---------------------------------------------------------------------------
# Story 8.4: Persona Load and Graceful Error Handling
# ---------------------------------------------------------------------------

class TestSystemPromptAssembly:
    """AC-1: System prompt is SOUL + '\\n\\n' + IDENTITY + '\\n\\n' + MEMORY."""

    def test_system_prompt_format(self):
        """Given persona files are loaded,
        When build_system_prompt() is called,
        Then the result contains SOUL + IDENTITY + MEMORY + batch instruction per ADR-017/022.
        """
        from agent_claude.persona import Persona

        p = Persona(soul="soul content", identity="identity content", memory="memory content")
        prompt = p.build_system_prompt()
        assert prompt.startswith("soul content\n\nidentity content\n\nmemory content")
        assert "## Output Format" in prompt
        assert "JSON array" in prompt

    def test_system_prompt_with_empty_components(self):
        """Given some persona files are missing (empty strings),
        When build_system_prompt() is called,
        Then the format is preserved with empty sections and batch instruction appended.
        """
        from agent_claude.persona import Persona

        p = Persona(soul="", identity="identity", memory="")
        prompt = p.build_system_prompt()
        assert prompt.startswith("\n\nidentity\n\n")
        assert "## Output Format" in prompt

    def test_system_prompt_passed_to_claude_api(
        self, mock_connection, config, persona, mock_claude_client
    ):
        """Given a TextMessage arrives,
        When the handler processes it,
        Then the system_prompt passed to claude_client.complete() is the assembled persona.
        """
        from agent_claude.loop import _make_handler

        handler = _make_handler(
            mock_connection, "main", mock_claude_client, persona, "donna", config["persona_path"],
        )

        msg = TextMessage(content="hello", publisher="user1")
        asyncio.get_event_loop().run_until_complete(handler(msg))

        call_kwargs = mock_claude_client.complete.call_args
        system_prompt_used = call_kwargs.kwargs.get("system_prompt") or call_kwargs.args[0]
        expected = persona.build_system_prompt()
        assert system_prompt_used == expected


class TestMemoryReloadBeforeEachCall:
    """AC-3: MEMORY.md is re-read from disk before each Claude API call."""

    def test_memory_reload_updates_system_prompt(self, tmp_path):
        """Given MEMORY.md changes between calls,
        When reload_memory() is called,
        Then the next build_system_prompt() reflects the new content.
        """
        from agent_claude.persona import Persona

        memory_file = tmp_path / "MEMORY.md"
        memory_file.write_text("original memory", encoding="utf-8")

        p = Persona(soul="soul", identity="identity", memory="original memory")
        assert "original memory" in p.build_system_prompt()

        # Simulate external update
        memory_file.write_text("updated memory", encoding="utf-8")
        p.reload_memory(str(tmp_path))
        assert "updated memory" in p.build_system_prompt()
        assert "original memory" not in p.build_system_prompt()

    async def test_reload_called_before_text_message_handler(
        self, mock_connection, mock_claude_client, config
    ):
        """Given a TextMessage arrives,
        When the handler runs,
        Then persona.reload_memory() is called before claude_client.complete().
        """
        from agent_claude.loop import _make_handler
        from agent_claude.persona import Persona

        call_order = []
        persona = MagicMock(spec=Persona)
        persona.build_system_prompt.return_value = "system prompt"

        def track_reload(path):
            call_order.append("reload_memory")

        persona.reload_memory.side_effect = track_reload

        async def track_complete(**kwargs):
            from agent_claude.claude_client import CompletionResult
            call_order.append("complete")
            return CompletionResult(text="reply", input_tokens=10, output_tokens=5)

        mock_claude_client.complete = track_complete

        handler = _make_handler(
            mock_connection, "main", mock_claude_client, persona, "donna", config["persona_path"],
        )

        msg = TextMessage(content="hello", publisher="user1")
        await handler(msg)

        assert call_order == ["reload_memory", "complete"]

    async def test_reload_called_before_cron_event_handler(
        self, mock_connection, mock_claude_client, config
    ):
        """Given a CronEvent arrives,
        When the handler runs,
        Then persona.reload_memory() is called before claude_client.complete().
        """
        from agent_claude.loop import _make_handler
        from agent_claude.persona import Persona

        call_order = []
        persona = MagicMock(spec=Persona)
        persona.build_system_prompt.return_value = "system prompt"

        def track_reload(path):
            call_order.append("reload_memory")

        persona.reload_memory.side_effect = track_reload

        async def track_complete(**kwargs):
            from agent_claude.claude_client import CompletionResult
            call_order.append("complete")
            return CompletionResult(text="reply", input_tokens=10, output_tokens=5)

        mock_claude_client.complete = track_complete

        handler = _make_handler(
            mock_connection, "main", mock_claude_client, persona, "donna", config["persona_path"],
        )

        msg = CronEvent(job_name="daily-check", triggered_at="2026-03-13T00:00:00Z")
        await handler(msg)

        assert call_order == ["reload_memory", "complete"]

    async def test_reload_called_before_skill_invocation_handler(
        self, mock_connection, mock_claude_client, config
    ):
        """Given a SkillInvocation arrives addressed to this agent,
        When the handler runs,
        Then persona.reload_memory() is called before claude_client.complete().
        """
        from agent_claude.loop import _make_handler
        from agent_claude.persona import Persona

        call_order = []
        persona = MagicMock(spec=Persona)
        persona.build_system_prompt.return_value = "system prompt"

        def track_reload(path):
            call_order.append("reload_memory")

        persona.reload_memory.side_effect = track_reload

        async def track_complete(**kwargs):
            from agent_claude.claude_client import CompletionResult
            call_order.append("complete")
            return CompletionResult(text="reply", input_tokens=10, output_tokens=5)

        mock_claude_client.complete = track_complete

        handler = _make_handler(
            mock_connection, "main", mock_claude_client, persona, "donna", config["persona_path"],
        )

        msg = SkillInvocation(
            invocation_id="inv-1",
            skill_name="summarize",
            input_payload="test input",
            target_agent="donna",
            publisher="orchestrator",
        )
        await handler(msg)

        assert call_order == ["reload_memory", "complete"]


class TestClaudeAuthErrorPropagation:
    """AC-5: Invalid API key -> ClaudeAuthError propagates to exit(1)."""

    async def test_auth_error_stored_as_fatal(
        self, mock_connection, config, persona
    ):
        """Given the Claude API returns AuthenticationError,
        When the handler processes a message,
        Then ClaudeAuthError is stored in _fatal_error and shutdown_event is set.
        """
        from agent_claude.errors import ClaudeAuthError
        from agent_claude.loop import _make_handler, _fatal_error, _shutdown_event

        claude_client = AsyncMock()
        claude_client.complete = AsyncMock(
            side_effect=ClaudeAuthError(
                "agent-claude: Claude API authentication failed -- check ANTHROPIC_API_KEY"
            )
        )

        handler = _make_handler(
            mock_connection, "main", claude_client, persona, "donna", config["persona_path"],
        )

        msg = TextMessage(content="hello", publisher="user1")
        with pytest.raises(ClaudeAuthError):
            await handler(msg)

        from agent_claude import loop
        assert loop._fatal_error is not None
        assert loop._shutdown_event.is_set()

    async def test_auth_error_propagates_from_message_loop(
        self, mock_connection, config, persona
    ):
        """Given ClaudeAuthError occurs during message processing,
        When run_message_loop() completes,
        Then it re-raises ClaudeAuthError.
        """
        from agent_claude.errors import ClaudeAuthError
        from agent_claude.loop import run_message_loop
        from agent_claude import loop

        claude_client = AsyncMock()
        claude_client.complete = AsyncMock(
            side_effect=ClaudeAuthError(
                "agent-claude: Claude API authentication failed -- check ANTHROPIC_API_KEY"
            )
        )

        # Simulate: subscribe stores the handler, then we call it manually
        handlers_stored = []

        async def fake_subscribe(stream, handler):
            handlers_stored.append(handler)

        mock_connection.subscribe = fake_subscribe

        # Start loop in background
        async def start_and_trigger():
            # Give loop time to subscribe
            await asyncio.sleep(0.01)
            # Trigger a message through the stored handler
            msg = TextMessage(content="hello", publisher="user1")
            try:
                await handlers_stored[0](msg)
            except ClaudeAuthError:
                pass  # SDK would catch this, but _fatal_error is already set

        trigger_task = asyncio.create_task(start_and_trigger())

        with pytest.raises(ClaudeAuthError):
            await run_message_loop(mock_connection, config, persona, claude_client)

        await trigger_task

    async def test_auth_error_message_content(self):
        """Given the API key is invalid,
        When ClaudeAuthError is raised,
        Then the message matches the expected format from AC-5.
        """
        from agent_claude.errors import ClaudeAuthError

        err = ClaudeAuthError(
            "agent-claude: Claude API authentication failed -- check ANTHROPIC_API_KEY"
        )
        assert "Claude API authentication failed" in str(err)
        assert "ANTHROPIC_API_KEY" in str(err)

    async def test_claude_client_raises_auth_error_on_authentication_error(self):
        """Given anthropic.AuthenticationError is raised,
        When ClaudeClient.complete() is called,
        Then it raises ClaudeAuthError.
        """
        import anthropic
        from agent_claude.claude_client import ClaudeClient
        from agent_claude.errors import ClaudeAuthError

        client = ClaudeClient(api_key="invalid-key")

        # Mock the internal _client to raise AuthenticationError
        mock_response = MagicMock()
        mock_response.status_code = 401
        mock_response.headers = {}
        auth_error = anthropic.AuthenticationError(
            message="Invalid API Key",
            response=mock_response,
            body={"error": {"message": "Invalid API Key"}},
        )

        with patch.object(client._client.messages, "create", side_effect=auth_error):
            with pytest.raises(ClaudeAuthError) as exc_info:
                await client.complete(
                    system_prompt="test",
                    user_message="hello",
                )
            assert "authentication failed" in str(exc_info.value)


class TestMissingApiKeyGracefulExit:
    """AC-4: Missing ANTHROPIC_API_KEY -> exit(1) with human-readable error."""

    def test_missing_api_key_exact_message(self):
        """Given ANTHROPIC_API_KEY is not set,
        When validate_env() is called,
        Then the error message matches the format from AC-4.
        """
        import os
        from agent_claude.errors import AgentConfigError
        from agent_claude.main import validate_env

        env = {
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            with pytest.raises(AgentConfigError) as exc_info:
                validate_env()
            msg = str(exc_info.value)
            assert "ANTHROPIC_API_KEY is not set" in msg
            assert "wh secrets init" in msg
            assert "ANTHROPIC_API_KEY environment variable" in msg

    def test_empty_api_key_same_as_missing(self):
        """Given ANTHROPIC_API_KEY is set to empty string,
        When validate_env() is called,
        Then it raises AgentConfigError just like missing.
        """
        import os
        from agent_claude.errors import AgentConfigError
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            with pytest.raises(AgentConfigError) as exc_info:
                validate_env()
            assert "ANTHROPIC_API_KEY" in str(exc_info.value)

    def test_whitespace_only_api_key_same_as_missing(self):
        """Given ANTHROPIC_API_KEY is set to whitespace only,
        When validate_env() is called,
        Then it raises AgentConfigError.
        """
        import os
        from agent_claude.errors import AgentConfigError
        from agent_claude.main import validate_env

        env = {
            "ANTHROPIC_API_KEY": "   ",
            "WH_URL": "tcp://127.0.0.1:5555",
            "WH_AGENT_NAME": "donna",
            "WH_STREAMS": "main",
        }
        with patch.dict(os.environ, env, clear=True):
            with pytest.raises(AgentConfigError) as exc_info:
                validate_env()
            assert "ANTHROPIC_API_KEY" in str(exc_info.value)


class TestEntryPointErrorHandling:
    """AC-4, AC-5: __main__.py catches errors and calls sys.exit(1)."""

    def test_main_catches_agent_config_error(self):
        """Given AgentConfigError is raised during startup,
        When main() runs,
        Then sys.exit(1) is called.
        """
        from agent_claude.errors import AgentConfigError

        with patch("agent_claude.__main__._run", side_effect=AgentConfigError("test error")):
            with patch("agent_claude.__main__.asyncio.run", side_effect=AgentConfigError("test error")):
                with pytest.raises(SystemExit) as exc_info:
                    from agent_claude.__main__ import main
                    main()
                assert exc_info.value.code == 1

    def test_main_catches_claude_auth_error(self):
        """Given ClaudeAuthError is raised during message processing,
        When main() runs,
        Then sys.exit(1) is called.
        """
        from agent_claude.errors import ClaudeAuthError

        with patch("agent_claude.__main__.asyncio.run", side_effect=ClaudeAuthError("auth failed")):
            with pytest.raises(SystemExit) as exc_info:
                from agent_claude.__main__ import main
                main()
            assert exc_info.value.code == 1


class TestMissingPersonaFilesGraceful:
    """AC-6: Missing persona files handled gracefully -- no exit."""

    def test_all_persona_files_missing_no_exit(self, tmp_path):
        """Given all three persona files are missing,
        When load_persona() is called,
        Then it returns a Persona with empty strings and does NOT raise.
        """
        from agent_claude.persona import load_persona

        persona = load_persona(str(tmp_path))
        assert persona.soul == ""
        assert persona.identity == ""
        assert persona.memory == ""
        # Verify MEMORY.md was created
        assert (tmp_path / "MEMORY.md").exists()

    def test_missing_soul_warns_and_continues(self, tmp_path, caplog):
        """Given SOUL.md is absent,
        When load_persona() is called,
        Then a warning is logged and soul is empty.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "IDENTITY.md").write_text("identity", encoding="utf-8")
        (tmp_path / "MEMORY.md").write_text("memory", encoding="utf-8")

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            persona = load_persona(str(tmp_path))

        assert persona.soul == ""
        assert any(r.levelno == logging.WARNING and "SOUL.md" in r.message for r in caplog.records)

    def test_missing_identity_warns_and_continues(self, tmp_path, caplog):
        """Given IDENTITY.md is absent,
        When load_persona() is called,
        Then a warning is logged and identity is empty.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "SOUL.md").write_text("soul", encoding="utf-8")
        (tmp_path / "MEMORY.md").write_text("memory", encoding="utf-8")

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            persona = load_persona(str(tmp_path))

        assert persona.identity == ""
        assert any(r.levelno == logging.WARNING and "IDENTITY.md" in r.message for r in caplog.records)

    def test_missing_memory_creates_empty_file(self, tmp_path, caplog):
        """Given MEMORY.md is absent,
        When load_persona() is called,
        Then an empty MEMORY.md file is created and a warning is logged.
        """
        from agent_claude.persona import load_persona

        (tmp_path / "SOUL.md").write_text("soul", encoding="utf-8")
        (tmp_path / "IDENTITY.md").write_text("identity", encoding="utf-8")

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            persona = load_persona(str(tmp_path))

        assert persona.memory == ""
        assert (tmp_path / "MEMORY.md").exists()
        assert (tmp_path / "MEMORY.md").read_text() == ""
        assert any(r.levelno == logging.WARNING and "MEMORY.md" in r.message for r in caplog.records)
