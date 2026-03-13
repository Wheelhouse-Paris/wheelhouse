"""Acceptance tests for Story 8.2: Stream Subscription and Message Processing Loop.

Tests verify all acceptance criteria for the message dispatch loop.

Acceptance Criteria:
  AC1: Agent subscribes to every stream in WH_STREAMS; info log lists them
  AC2: TextMessage content passed to Claude API with persona as system prompt
  AC3: CronEvent rendered as structured prompt and passed to Claude API
  AC4: SkillInvocation (for us) -> SkillProgress published + Claude API call
  AC5: Claude API timeout (60s) -> loop continues, error logged
  AC6: Self-echo filter drops TextMessage where publisher == agent_name
  AC7: SkillInvocation for other agent dropped silently
  AC8: MEMORY.md re-read before each Claude API call
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
    yield
    loop._pending_tasks.clear()
    loop._shutdown_event = asyncio.Event()


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
        from agent_claude.loop import run_message_loop, _shutdown_event

        # Start the loop in background and stop it immediately
        _shutdown_event.set()
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
        from agent_claude.loop import run_message_loop, _shutdown_event

        _shutdown_event.set()
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

        # SkillProgress should have been published
        mock_connection.publish.assert_called_once()
        pub_args = mock_connection.publish.call_args
        assert pub_args.args[0] == "main"
        progress = pub_args.args[1]
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
