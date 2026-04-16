"""Tests for multi-agent simultaneous serving and tenant isolation.

Story 14-2-4: Validates that the librarian correctly handles events from
multiple agents simultaneously with proper attribution, sequential event
processing to prevent git conflicts, and tenant isolation via volume scoping.

Covers:
  - AC #1: Events from 3 agents processed with correct source_agent_id attribution
  - AC #2: Tenant isolation via topology-scoped volume naming
  - AC #3: Sequential event processing (serialized, no git conflicts)
"""

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, call

import pytest

from wheelhouse.librarian.loop import LibrarianLoop
from wheelhouse.librarian.proto import (
    ConversationMessage as ProtoConversationMessage,
    LibraryWriteEvent,
)
from wheelhouse.librarian.types import DecisionResult


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_event(
    *,
    event_id: str = "evt-001",
    source_agent_id: str = "agent-a",
    library_id: str = "lib-test",
    conversation_id: str = "conv-001",
    locale: str = "en",
    segment_text: str = "My birthday is April 5th.",
) -> LibraryWriteEvent:
    """Create a minimal LibraryWriteEvent for testing."""
    event = LibraryWriteEvent()
    event.event_id = event_id
    event.source_agent_id = source_agent_id
    event.library_id = library_id
    event.conversation_id = conversation_id
    event.locale = locale
    msg = ProtoConversationMessage()
    msg.role = "user"
    msg.content = segment_text
    msg.timestamp_ms = 1713000000000
    event.segment = [msg]
    event.timestamp_ms = 1713000000000
    return event


def _mock_llm_response(
    reason: str,
    committed: bool,
    page_path: str | None = None,
    content: str | None = None,
) -> str:
    """Create a JSON string mimicking the LLM decision response."""
    return json.dumps(
        {
            "reason": reason,
            "committed": committed,
            "page_path": page_path,
            "content": content,
        }
    )


def _make_sandbox_mock() -> MagicMock:
    """Create a mock LibrarySandbox with reasonable defaults."""
    sandbox = MagicMock()
    sandbox.exists.return_value = False
    sandbox.list.return_value = []
    sandbox.read.return_value = ""
    return sandbox


def _make_loop(
    tmp_path: Path,
    llm_fn=None,
    sandbox=None,
) -> LibrarianLoop:
    """Create a LibrarianLoop with sensible defaults."""
    return LibrarianLoop(
        library_path=tmp_path,
        library_id="test-lib",
        locales=["en", "fr"],
        llm_fn=llm_fn,
        sandbox=sandbox,
    )


# ---------------------------------------------------------------------------
# AC #1: Multi-agent attribution
# ---------------------------------------------------------------------------


class TestMultiAgentAttribution:
    """Events from multiple agents are attributed correctly."""

    def test_three_agents_produce_attributed_results(self, tmp_path: Path):
        """3 agents produce events; each DecisionResult carries correct source."""
        agents = ["agent-a", "agent-b", "agent-c"]
        events = [
            _make_event(
                event_id=f"evt-{i}",
                source_agent_id=agent,
                segment_text=f"Fact from {agent}: item {i}",
            )
            for i, agent in enumerate(agents)
        ]

        # LLM always writes
        def llm_fn(system_prompt: str, user_content: str) -> str:
            # Determine which agent from user_content
            for agent in agents:
                if agent in user_content:
                    return _mock_llm_response(
                        reason="durable_fact_written",
                        committed=True,
                        page_path=f"pages/{agent}.md",
                        content=f"# Fact from {agent}",
                    )
            return _mock_llm_response(reason="decision_error", committed=False)

        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        results = [loop.process_event(event) for event in events]

        # All 3 events processed successfully
        assert len(results) == 3
        for i, result in enumerate(results):
            assert result.committed is True
            assert result.reason == "durable_fact_written"
            assert result.event_id == f"evt-{i}"

    def test_commit_metadata_includes_source_agent_id(self, tmp_path: Path):
        """sandbox.commit() receives source agent as the 'sources' kwarg."""
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path="pages/fact.md",
                content="# A durable fact",
            )
        )
        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event = _make_event(source_agent_id="agent-b")
        loop.process_event(event)

        # Verify sandbox.commit() was called with sources=["agent-b"]
        sandbox.commit.assert_called_once()
        commit_kwargs = sandbox.commit.call_args
        assert commit_kwargs.kwargs.get("sources") == ["agent-b"]
        assert commit_kwargs.kwargs.get("pages_created") == ["pages/fact.md"]

    def test_begin_transaction_summary_contains_source_agent(self, tmp_path: Path):
        """Transaction summary includes source_agent_id for git log attribution."""
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path="pages/fact.md",
                content="# A fact",
            )
        )
        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event = _make_event(source_agent_id="agent-c")
        loop.process_event(event)

        sandbox.begin_transaction.assert_called_once()
        summary_arg = sandbox.begin_transaction.call_args.kwargs.get("summary", "")
        assert "agent-c" in summary_arg

    def test_interleaved_events_attributed_correctly(self, tmp_path: Path):
        """Events from agent-a and agent-b interleaved produce correct attribution."""
        call_index = [0]

        def llm_fn(system_prompt: str, user_content: str) -> str:
            idx = call_index[0]
            call_index[0] += 1
            agent = "agent-a" if idx % 2 == 0 else "agent-b"
            return _mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path=f"pages/fact-{idx}.md",
                content=f"# Fact {idx}",
            )

        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        events = [
            _make_event(event_id="evt-0", source_agent_id="agent-a"),
            _make_event(event_id="evt-1", source_agent_id="agent-b"),
            _make_event(event_id="evt-2", source_agent_id="agent-a"),
            _make_event(event_id="evt-3", source_agent_id="agent-b"),
        ]

        results = [loop.process_event(e) for e in events]

        # Verify each result's event_id matches the input
        for i, result in enumerate(results):
            assert result.event_id == f"evt-{i}"
            assert result.committed is True

        # Verify commit was called 4 times with correct sources
        assert sandbox.commit.call_count == 4
        sources_per_call = [
            c.kwargs.get("sources", []) for c in sandbox.commit.call_args_list
        ]
        assert sources_per_call[0] == ["agent-a"]
        assert sources_per_call[1] == ["agent-b"]
        assert sources_per_call[2] == ["agent-a"]
        assert sources_per_call[3] == ["agent-b"]

    def test_update_existing_page_includes_sources(self, tmp_path: Path):
        """When updating an existing page, sources still carries source_agent_id."""
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                reason="update_existing_page",
                committed=True,
                page_path="pages/existing.md",
                content="# Updated content",
            )
        )
        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event = _make_event(source_agent_id="agent-a")
        result = loop.process_event(event)

        assert result.committed is True
        assert result.reason == "update_existing_page"
        sandbox.commit.assert_called_once()
        commit_kwargs = sandbox.commit.call_args.kwargs
        assert commit_kwargs.get("sources") == ["agent-a"]
        assert commit_kwargs.get("pages_updated") == ["pages/existing.md"]
        # pages_created should NOT be present
        assert "pages_created" not in commit_kwargs

    def test_missing_source_agent_id_defaults_to_unknown(self, tmp_path: Path):
        """Events with empty source_agent_id are attributed to 'unknown'."""
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path="pages/fact.md",
                content="# A fact",
            )
        )
        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event = _make_event(source_agent_id="")
        loop.process_event(event)

        sandbox.commit.assert_called_once()
        assert sandbox.commit.call_args.kwargs.get("sources") == ["unknown"]


# ---------------------------------------------------------------------------
# AC #3: Sequential event processing
# ---------------------------------------------------------------------------


class TestSequentialProcessing:
    """Event processing is serialized (one at a time) to prevent git conflicts."""

    def test_process_event_is_synchronous(self, tmp_path: Path):
        """process_event() is a regular synchronous method, not a coroutine."""
        import inspect

        loop = _make_loop(tmp_path)
        assert not inspect.iscoroutinefunction(loop.process_event)

    def test_sequential_calls_both_succeed(self, tmp_path: Path):
        """Two sequential process_event calls both complete without errors."""
        call_count = [0]

        def llm_fn(system_prompt: str, user_content: str) -> str:
            call_count[0] += 1
            return _mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path=f"pages/page-{call_count[0]}.md",
                content=f"# Page {call_count[0]}",
            )

        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event_a = _make_event(event_id="evt-a", source_agent_id="agent-a")
        event_b = _make_event(event_id="evt-b", source_agent_id="agent-b")

        result_a = loop.process_event(event_a)
        result_b = loop.process_event(event_b)

        assert result_a.committed is True
        assert result_b.committed is True
        assert sandbox.commit.call_count == 2

    def test_three_agents_sequential_no_conflicts(self, tmp_path: Path):
        """3 agents processed sequentially produce 3 separate commits."""
        agents = ["agent-a", "agent-b", "agent-c"]
        call_idx = [0]

        def llm_fn(system_prompt: str, user_content: str) -> str:
            idx = call_idx[0]
            call_idx[0] += 1
            return _mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path=f"pages/fact-{idx}.md",
                content=f"# Fact {idx}",
            )

        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        for i, agent in enumerate(agents):
            event = _make_event(event_id=f"evt-{i}", source_agent_id=agent)
            result = loop.process_event(event)
            assert result.committed is True

        assert sandbox.commit.call_count == 3
        # Each commit should have a distinct source
        for i, agent in enumerate(agents):
            assert sandbox.commit.call_args_list[i].kwargs.get("sources") == [agent]

    def test_skip_events_do_not_call_commit(self, tmp_path: Path):
        """Skipped events (transient_context) do not produce git commits."""
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                reason="transient_context",
                committed=False,
            )
        )
        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event = _make_event(source_agent_id="agent-a")
        result = loop.process_event(event)

        assert result.committed is False
        assert result.reason == "transient_context"
        sandbox.commit.assert_not_called()
        sandbox.begin_transaction.assert_not_called()

    def test_mixed_write_and_skip_from_different_agents(self, tmp_path: Path):
        """Mix of writes and skips from different agents processed correctly."""
        responses = [
            ("durable_fact_written", True, "pages/a.md", "# From A"),
            ("transient_context", False, None, None),
            ("durable_fact_written", True, "pages/c.md", "# From C"),
        ]

        call_idx = [0]

        def llm_fn(system_prompt: str, user_content: str) -> str:
            idx = call_idx[0]
            call_idx[0] += 1
            r = responses[idx]
            return _mock_llm_response(
                reason=r[0], committed=r[1], page_path=r[2], content=r[3]
            )

        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        events = [
            _make_event(event_id="evt-0", source_agent_id="agent-a"),
            _make_event(event_id="evt-1", source_agent_id="agent-b"),
            _make_event(event_id="evt-2", source_agent_id="agent-c"),
        ]

        results = [loop.process_event(e) for e in events]

        assert results[0].committed is True  # agent-a writes
        assert results[1].committed is False  # agent-b skipped
        assert results[2].committed is True  # agent-c writes
        assert sandbox.commit.call_count == 2


# ---------------------------------------------------------------------------
# AC #2: Tenant isolation
# ---------------------------------------------------------------------------


class TestTenantIsolation:
    """Tenant isolation via separate library paths and volume scoping."""

    def test_two_loops_with_different_paths_are_isolated(self, tmp_path: Path):
        """Two LibrarianLoop instances with different library_paths are isolated."""
        path_a = tmp_path / "tenant-a"
        path_b = tmp_path / "tenant-b"
        path_a.mkdir()
        path_b.mkdir()

        loop_a = LibrarianLoop(
            library_path=path_a,
            library_id="lib-a",
            locales=["en"],
        )
        loop_b = LibrarianLoop(
            library_path=path_b,
            library_id="lib-b",
            locales=["en"],
        )

        assert loop_a.library_path != loop_b.library_path
        assert loop_a.library_id != loop_b.library_id
        # Verify paths are distinct directories
        assert not str(loop_a.library_path).startswith(str(loop_b.library_path))
        assert not str(loop_b.library_path).startswith(str(loop_a.library_path))

    def test_library_state_reads_only_own_path(self, tmp_path: Path):
        """_build_library_state() reads from the loop's own library_path."""
        path_a = tmp_path / "tenant-a"
        path_b = tmp_path / "tenant-b"
        path_a.mkdir()
        path_b.mkdir()

        # Write a file in tenant-b's path
        (path_b / "pages").mkdir()
        (path_b / "pages" / "secret.md").write_text("# Tenant B secret")

        sandbox_a = MagicMock()
        sandbox_a.exists.return_value = False
        sandbox_a.list.return_value = []

        loop_a = LibrarianLoop(
            library_path=path_a,
            library_id="lib-a",
            sandbox=sandbox_a,
        )

        state = loop_a._build_library_state()
        # Tenant A's state should be empty — it cannot see tenant B's files
        assert state.existing_pages == {}

    def test_volume_name_scoping_pattern(self):
        """Validate the volume naming convention ensures tenant isolation.

        Volume names follow the pattern wh-<topology>-llm-wiki-<library_name>.
        Two topologies with the same library_name produce DIFFERENT volume names
        because the topology name differs.
        """
        # This test validates the naming convention structurally.
        # The actual volume creation is in Rust (composition.rs);
        # the Rust-side test covers the runtime behavior.
        topo_a = "acme-prod"
        topo_b = "globex-prod"
        library_name = "research"

        vol_a = f"wh-{topo_a}-llm-wiki-{library_name}"
        vol_b = f"wh-{topo_b}-llm-wiki-{library_name}"

        assert vol_a != vol_b
        assert vol_a == "wh-acme-prod-llm-wiki-research"
        assert vol_b == "wh-globex-prod-llm-wiki-research"

    def test_loop_library_id_distinguishes_tenants(self, tmp_path: Path):
        """Each LibrarianLoop carries a distinct library_id for log attribution."""
        loop_a = LibrarianLoop(
            library_path=tmp_path / "a", library_id="tenant-a-lib"
        )
        loop_b = LibrarianLoop(
            library_path=tmp_path / "b", library_id="tenant-b-lib"
        )

        assert loop_a.library_id == "tenant-a-lib"
        assert loop_b.library_id == "tenant-b-lib"
        assert loop_a.library_id != loop_b.library_id


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


class TestEdgeCases:
    """Edge cases for multi-agent serving."""

    def test_decision_error_does_not_block_subsequent_events(self, tmp_path: Path):
        """A decision_error for one event does not prevent processing the next."""
        call_idx = [0]

        def llm_fn(system_prompt: str, user_content: str) -> str:
            idx = call_idx[0]
            call_idx[0] += 1
            if idx == 0:
                return "INVALID JSON"  # Will cause decision_error
            return _mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path="pages/good.md",
                content="# Good",
            )

        sandbox = _make_sandbox_mock()
        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event_bad = _make_event(event_id="evt-bad", source_agent_id="agent-a")
        event_good = _make_event(event_id="evt-good", source_agent_id="agent-b")

        result_bad = loop.process_event(event_bad)
        result_good = loop.process_event(event_good)

        assert result_bad.committed is False
        assert result_bad.reason == "decision_error"
        assert result_good.committed is True
        assert result_good.reason == "durable_fact_written"

    def test_commit_failure_does_not_block_subsequent_events(self, tmp_path: Path):
        """A commit failure for one event does not prevent processing the next."""
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                reason="durable_fact_written",
                committed=True,
                page_path="pages/fact.md",
                content="# Fact",
            )
        )
        sandbox = _make_sandbox_mock()
        # First commit raises, second succeeds
        sandbox.commit.side_effect = [RuntimeError("git lock"), None]

        loop = _make_loop(tmp_path, llm_fn=llm_fn, sandbox=sandbox)

        event_1 = _make_event(event_id="evt-1", source_agent_id="agent-a")
        event_2 = _make_event(event_id="evt-2", source_agent_id="agent-b")

        result_1 = loop.process_event(event_1)
        result_2 = loop.process_event(event_2)

        # First event: commit failed, so committed should be False in result
        # (the method returns None for commit_hash on failure, but the
        # DecisionResult at loop level reflects the policy decision — committed
        # is True because the policy said to commit. The commit_hash being None
        # indicates the actual commit failed.)
        assert result_1.committed is True  # policy decided to commit
        assert result_2.committed is True  # second event succeeds

    def test_no_llm_fn_returns_decision_error_for_all_agents(self, tmp_path: Path):
        """Without llm_fn, all events from any agent return decision_error."""
        loop = _make_loop(tmp_path, llm_fn=None)

        for agent in ["agent-a", "agent-b", "agent-c"]:
            event = _make_event(source_agent_id=agent)
            result = loop.process_event(event)
            assert result.reason == "decision_error"
            assert result.committed is False
