"""Tests for structured decision logging (story 14-1-5).

Covers:
  - AC-1: Structured JSON log on commit
  - AC-2: Structured JSON log on skip
  - AC-3: Decision timeout logging
  - AC-4: Decision error logging (unparseable response)
  - AC-5: Step spans for full trace recovery
  - AC-6: SkillResult emission for metering
  - NFR20: Snippet truncation to 200 chars
"""

from __future__ import annotations

import concurrent.futures
import json
import logging
import time
from unittest.mock import MagicMock, patch

import pytest

from wheelhouse.librarian.decision_log import (
    DecisionLogEntry,
    SpanTimer,
    build_snippet,
    emit_decision_log,
)
from wheelhouse.librarian.loop import LibrarianLoop, _LLM_TIMEOUT_S
from wheelhouse.librarian.proto import (
    ConversationMessage,
    LibraryWriteEvent,
)
from wheelhouse.librarian.types import DecisionResult


# ── Fixtures ──────────────────────────────────────────────────────────


def _make_event(
    event_id: str = "evt-001",
    source_agent_id: str = "agent-a",
    library_id: str = "research",
    conversation_id: str = "conv-123",
    locale: str = "en",
    user_content: str = "I work at Acme Corp as a senior engineer",
    assistant_content: str = "Noted, you work at Acme Corp.",
) -> LibraryWriteEvent:
    now_ms = int(time.time() * 1000)
    return LibraryWriteEvent(
        event_id=event_id,
        source_agent_id=source_agent_id,
        library_id=library_id,
        conversation_id=conversation_id,
        timestamp_ms=now_ms,
        segment=[
            ConversationMessage(role="user", content=user_content, timestamp_ms=now_ms),
            ConversationMessage(
                role="assistant", content=assistant_content, timestamp_ms=now_ms
            ),
        ],
        locale=locale,
    )


def _make_llm_fn(response: dict | str | None = None, delay: float = 0.0):
    """Create a mock llm_fn that returns a JSON response."""

    def llm_fn(system_prompt: str, user_content: str) -> str:
        if delay > 0:
            time.sleep(delay)
        if response is None:
            return json.dumps(
                {
                    "reason": "durable_fact_written",
                    "committed": True,
                    "page_path": "pages/acme-corp.md",
                    "content": "---\nsource: agent-a\n---\nWorks at Acme Corp",
                }
            )
        if isinstance(response, dict):
            return json.dumps(response)
        return response

    return llm_fn


def _make_sandbox(commit_succeeds: bool = True):
    """Create a mock LibrarySandbox."""
    sandbox = MagicMock()
    sandbox.exists.return_value = False
    sandbox.list.return_value = []
    if not commit_succeeds:
        sandbox.commit.side_effect = RuntimeError("commit failed")
    return sandbox


def _make_loop(
    llm_fn=None,
    sandbox=None,
    dedup=None,
    publish_skill_result=None,
    library_path: str = "/tmp/test-library",
) -> LibrarianLoop:
    """Create a LibrarianLoop with sensible test defaults."""
    return LibrarianLoop(
        library_path=library_path,
        library_id="research",
        locales=["en", "fr"],
        llm_fn=llm_fn,
        sandbox=sandbox,
        dedup=dedup,
        publish_skill_result=publish_skill_result,
        agent_name="librarian-research",
    )


# ── Tests: build_snippet ──────────────────────────────────────────────


class TestBuildSnippet:
    def test_short_content_unchanged(self):
        result = build_snippet(["Hello world"])
        assert result == "Hello world"

    def test_multiple_segments_joined(self):
        result = build_snippet(["Hello", "World"])
        assert result == "Hello | World"

    def test_truncates_to_200_chars(self):
        long_text = "A" * 300
        result = build_snippet([long_text])
        assert len(result) == 200

    def test_truncates_combined_to_200(self):
        result = build_snippet(["A" * 150, "B" * 150])
        assert len(result) == 200
        # Should be "AAA...AAA | BBB...BBB" truncated
        assert result.startswith("A")

    def test_empty_segments(self):
        result = build_snippet([])
        assert result == ""

    def test_custom_max_len(self):
        result = build_snippet(["Hello World"], max_len=5)
        assert result == "Hello"


# ── Tests: SpanTimer ──────────────────────────────────────────────────


class TestSpanTimer:
    def test_records_span(self):
        timer = SpanTimer()
        with timer.span("test_span"):
            time.sleep(0.01)
        assert len(timer.spans) == 1
        assert timer.spans[0]["name"] == "test_span"
        assert timer.spans[0]["duration_ms"] >= 0

    def test_multiple_spans(self):
        timer = SpanTimer()
        with timer.span("first"):
            pass
        with timer.span("second"):
            pass
        assert len(timer.spans) == 2
        assert timer.spans[0]["name"] == "first"
        assert timer.spans[1]["name"] == "second"

    def test_total_ms(self):
        timer = SpanTimer()
        with timer.span("a"):
            time.sleep(0.01)
        with timer.span("b"):
            time.sleep(0.01)
        assert timer.total_ms >= 0


# ── Tests: emit_decision_log ──────────────────────────────────────────


class TestEmitDecisionLog:
    def test_emits_json_to_logger(self, caplog):
        entry = DecisionLogEntry(
            schema_version=1,
            event_id="evt-001",
            library_id="research",
            source_agent_id="agent-a",
            conversation_id="conv-123",
            reason="durable_fact_written",
            committed=True,
            commit_hash="abc123",
            locale="en",
            tokens_consumed=450,
            snippet="User works at Acme",
            timestamp="2026-04-16T10:30:00+00:00",
            duration_ms=2340,
            step_spans=[
                {"name": "dedup_check", "duration_ms": 1},
                {"name": "llm_call", "duration_ms": 2100},
            ],
        )
        with caplog.at_level(logging.INFO, logger="wheelhouse.librarian.decision"):
            emit_decision_log(entry)

        assert len(caplog.records) == 1
        parsed = json.loads(caplog.records[0].message)
        assert parsed["schema_version"] == 1
        assert parsed["event_id"] == "evt-001"
        assert parsed["committed"] is True
        assert parsed["commit_hash"] == "abc123"
        assert parsed["reason"] == "durable_fact_written"
        assert len(parsed["step_spans"]) == 2

    def test_skip_entry_has_null_commit_hash(self, caplog):
        entry = DecisionLogEntry(
            schema_version=1,
            event_id="evt-002",
            library_id="research",
            source_agent_id="agent-a",
            conversation_id="conv-456",
            reason="transient_context",
            committed=False,
            commit_hash=None,
            locale="en",
            tokens_consumed=100,
            snippet="Hi there",
            timestamp="2026-04-16T10:31:00+00:00",
            duration_ms=500,
            step_spans=[],
        )
        with caplog.at_level(logging.INFO, logger="wheelhouse.librarian.decision"):
            emit_decision_log(entry)

        parsed = json.loads(caplog.records[0].message)
        assert parsed["committed"] is False
        assert parsed["commit_hash"] is None


# ── Tests: LibrarianLoop structured logging ───────────────────────────


class TestProcessEventDecisionLog:
    """Test that process_event() emits structured decision logs."""

    def test_commit_emits_structured_log(self, caplog, tmp_path):
        """AC-1: Committed decision produces structured JSON log."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        # Mock git rev-parse for commit hash extraction
        with (
            caplog.at_level(logging.INFO, logger="wheelhouse.librarian.decision"),
            patch("wheelhouse.librarian.loop.subprocess") as mock_sp,
        ):
            mock_sp.run.return_value = MagicMock(
                returncode=0, stdout="abc123def456\n"
            )
            result = loop.process_event(event)

        assert result.committed is True
        assert result.reason == "durable_fact_written"
        assert result.commit_hash == "abc123def456"

        # Find the JSON decision log entry
        json_records = [
            r
            for r in caplog.records
            if r.name == "wheelhouse.librarian.decision"
        ]
        assert len(json_records) == 1
        parsed = json.loads(json_records[0].message)
        assert parsed["schema_version"] == 1
        assert parsed["event_id"] == "evt-001"
        assert parsed["library_id"] == "research"
        assert parsed["source_agent_id"] == "agent-a"
        assert parsed["conversation_id"] == "conv-123"
        assert parsed["reason"] == "durable_fact_written"
        assert parsed["committed"] is True
        assert parsed["commit_hash"] == "abc123def456"
        assert parsed["locale"] == "en"
        assert isinstance(parsed["duration_ms"], int)
        assert isinstance(parsed["step_spans"], list)
        assert isinstance(parsed["snippet"], str)
        assert len(parsed["snippet"]) <= 200
        assert "timestamp" in parsed

    def test_skip_emits_structured_log(self, caplog, tmp_path):
        """AC-2: Skipped decision produces structured JSON log."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn(
            {"reason": "transient_context", "committed": False}
        )
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with caplog.at_level(logging.INFO, logger="wheelhouse.librarian.decision"):
            result = loop.process_event(event)

        assert result.committed is False
        assert result.reason == "transient_context"

        json_records = [
            r
            for r in caplog.records
            if r.name == "wheelhouse.librarian.decision"
        ]
        assert len(json_records) == 1
        parsed = json.loads(json_records[0].message)
        assert parsed["committed"] is False
        assert parsed["commit_hash"] is None
        assert parsed["reason"] == "transient_context"

    def test_decision_error_emits_structured_log(self, caplog, tmp_path):
        """AC-4: Unparseable LLM response logs decision_error."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn("this is not json at all")
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with caplog.at_level(logging.INFO, logger="wheelhouse.librarian.decision"):
            result = loop.process_event(event)

        assert result.reason == "decision_error"
        assert result.committed is False

        json_records = [
            r
            for r in caplog.records
            if r.name == "wheelhouse.librarian.decision"
        ]
        assert len(json_records) == 1
        parsed = json.loads(json_records[0].message)
        assert parsed["reason"] == "decision_error"
        assert parsed["committed"] is False

    def test_dedup_skip_emits_structured_log(self, caplog, tmp_path):
        """Dedup skip path also emits structured log."""
        from wheelhouse.librarian.dedup import DedupCache

        dedup = DedupCache(maxsize=100)
        dedup.add("evt-dup")

        loop = _make_loop(
            llm_fn=_make_llm_fn(),
            dedup=dedup,
            library_path=str(tmp_path),
        )
        event = _make_event(event_id="evt-dup")

        with caplog.at_level(logging.INFO, logger="wheelhouse.librarian.decision"):
            result = loop.process_event(event)

        assert result.reason == "dedup_skipped"

        json_records = [
            r
            for r in caplog.records
            if r.name == "wheelhouse.librarian.decision"
        ]
        assert len(json_records) == 1
        parsed = json.loads(json_records[0].message)
        assert parsed["reason"] == "dedup_skipped"
        assert parsed["committed"] is False


# ── Tests: Step Spans ─────────────────────────────────────────────────


class TestStepSpans:
    """AC-5: Step spans for full trace recovery."""

    def test_commit_has_expected_spans(self, tmp_path):
        """Committed decision includes dedup_check, prompt_load, llm_call, git_commit."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with patch("wheelhouse.librarian.loop.subprocess") as mock_sp:
            mock_sp.run.return_value = MagicMock(returncode=0, stdout="abc\n")
            result = loop.process_event(event)

        span_names = [s["name"] for s in result.step_spans]
        assert "dedup_check" in span_names
        assert "prompt_load" in span_names
        assert "llm_call" in span_names
        assert "git_commit" in span_names

    def test_skip_has_spans_without_git(self, tmp_path):
        """Skipped decision has dedup_check, prompt_load, llm_call but no git_commit."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn(
            {"reason": "transient_context", "committed": False}
        )
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        result = loop.process_event(event)

        span_names = [s["name"] for s in result.step_spans]
        assert "dedup_check" in span_names
        assert "prompt_load" in span_names
        assert "llm_call" in span_names
        assert "git_commit" not in span_names

    def test_dedup_skip_has_only_dedup_span(self, tmp_path):
        """Dedup skip only has dedup_check span."""
        from wheelhouse.librarian.dedup import DedupCache

        dedup = DedupCache(maxsize=100)
        dedup.add("evt-dup")

        loop = _make_loop(
            llm_fn=_make_llm_fn(),
            dedup=dedup,
            library_path=str(tmp_path),
        )
        event = _make_event(event_id="evt-dup")
        result = loop.process_event(event)

        span_names = [s["name"] for s in result.step_spans]
        assert span_names == ["dedup_check"]

    def test_duration_ms_populated(self, tmp_path):
        """duration_ms is a positive integer."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        result = loop.process_event(event)
        assert result.duration_ms >= 0
        assert isinstance(result.duration_ms, int)


# ── Tests: Decision Timeout ───────────────────────────────────────────


class TestDecisionTimeout:
    """AC-3: LLM call timeout produces decision_timeout reason."""

    def test_timeout_produces_decision_timeout(self, tmp_path):
        """LLM call exceeding timeout returns decision_timeout."""

        def slow_llm(system_prompt: str, user_content: str) -> str:
            time.sleep(15)  # Exceeds _LLM_TIMEOUT_S
            return json.dumps({"reason": "durable_fact_written", "committed": True})

        sandbox = _make_sandbox()
        loop = _make_loop(
            llm_fn=slow_llm,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        # Patch the timeout to be very short for test speed
        with patch.object(
            type(loop),
            "_call_decide_with_timeout",
            side_effect=concurrent.futures.TimeoutError("timed out"),
        ):
            result = loop.process_event(event)

        assert result.reason == "decision_timeout"
        assert result.committed is False


# ── Tests: Commit Hash ────────────────────────────────────────────────


class TestCommitHash:
    """AC-1: commit_hash populated after successful write."""

    def test_commit_hash_populated(self, tmp_path):
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with patch("wheelhouse.librarian.loop.subprocess") as mock_sp:
            mock_sp.run.return_value = MagicMock(
                returncode=0, stdout="deadbeef1234\n"
            )
            result = loop.process_event(event)

        assert result.commit_hash == "deadbeef1234"
        assert result.committed is True

    def test_commit_hash_none_on_skip(self, tmp_path):
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn(
            {"reason": "transient_context", "committed": False}
        )
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()
        result = loop.process_event(event)
        assert result.commit_hash is None

    def test_commit_hash_none_on_git_failure(self, tmp_path):
        """If git rev-parse fails, commit_hash is None but commit succeeded."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with patch("wheelhouse.librarian.loop.subprocess") as mock_sp:
            mock_sp.run.return_value = MagicMock(returncode=1, stdout="")
            result = loop.process_event(event)

        # Commit succeeded (sandbox.commit didn't raise) but hash extraction failed
        assert result.committed is True
        assert result.commit_hash is None


# ── Tests: SkillResult Emission ───────────────────────────────────────


class TestSkillResultEmission:
    """AC-6: SkillResult emitted for metering."""

    def test_skill_result_emitted_on_commit(self, tmp_path):
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        mock_publish = MagicMock()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            publish_skill_result=mock_publish,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with patch("wheelhouse.librarian.loop.subprocess") as mock_sp:
            mock_sp.run.return_value = MagicMock(returncode=0, stdout="abc\n")
            loop.process_event(event)

        mock_publish.assert_called_once()
        call_kwargs = mock_publish.call_args[1]
        assert call_kwargs["skill_name"] == "librarian_decide"
        assert call_kwargs["success"] is True
        assert call_kwargs["output"] == "durable_fact_written"
        assert call_kwargs["invocation_id"] == "evt-001"
        assert call_kwargs["library_id"] == "research"
        assert call_kwargs["agent_id"] == "librarian-research"

    def test_skill_result_emitted_on_skip(self, tmp_path):
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn(
            {"reason": "transient_context", "committed": False}
        )
        mock_publish = MagicMock()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            publish_skill_result=mock_publish,
            library_path=str(tmp_path),
        )
        event = _make_event()
        loop.process_event(event)

        mock_publish.assert_called_once()
        call_kwargs = mock_publish.call_args[1]
        assert call_kwargs["success"] is False
        assert call_kwargs["output"] == "transient_context"

    def test_no_publish_when_callback_none(self, tmp_path):
        """No error when publish_skill_result is None."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            publish_skill_result=None,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with patch("wheelhouse.librarian.loop.subprocess") as mock_sp:
            mock_sp.run.return_value = MagicMock(returncode=0, stdout="abc\n")
            result = loop.process_event(event)

        # Should not raise
        assert result.committed is True

    def test_publish_failure_does_not_crash(self, tmp_path):
        """SkillResult publish failure is swallowed."""
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn()
        mock_publish = MagicMock(side_effect=RuntimeError("publish failed"))
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            publish_skill_result=mock_publish,
            library_path=str(tmp_path),
        )
        event = _make_event()

        with patch("wheelhouse.librarian.loop.subprocess") as mock_sp:
            mock_sp.run.return_value = MagicMock(returncode=0, stdout="abc\n")
            result = loop.process_event(event)

        assert result.committed is True
        mock_publish.assert_called_once()


# ── Tests: Snippet PII Boundary ───────────────────────────────────────


class TestSnippetPIIBoundary:
    """NFR20: Snippet in decision log capped at 200 chars."""

    def test_long_segment_truncated_in_log(self, caplog, tmp_path):
        sandbox = _make_sandbox()
        llm_fn = _make_llm_fn(
            {"reason": "transient_context", "committed": False}
        )
        loop = _make_loop(
            llm_fn=llm_fn,
            sandbox=sandbox,
            library_path=str(tmp_path),
        )
        # Create event with very long content
        event = _make_event(user_content="X" * 300, assistant_content="Y" * 300)

        with caplog.at_level(logging.INFO, logger="wheelhouse.librarian.decision"):
            loop.process_event(event)

        json_records = [
            r
            for r in caplog.records
            if r.name == "wheelhouse.librarian.decision"
        ]
        assert len(json_records) == 1
        parsed = json.loads(json_records[0].message)
        assert len(parsed["snippet"]) <= 200


# ── Tests: DecisionResult new fields ──────────────────────────────────


class TestDecisionResultNewFields:
    """Verify new fields on DecisionResult."""

    def test_default_values(self):
        r = DecisionResult()
        assert r.commit_hash is None
        assert r.duration_ms == 0
        assert r.step_spans == []

    def test_fields_populated(self):
        r = DecisionResult(
            reason="durable_fact_written",
            committed=True,
            event_id="evt-001",
            commit_hash="abc123",
            duration_ms=500,
            step_spans=[{"name": "llm_call", "duration_ms": 400}],
        )
        assert r.commit_hash == "abc123"
        assert r.duration_ms == 500
        assert len(r.step_spans) == 1
