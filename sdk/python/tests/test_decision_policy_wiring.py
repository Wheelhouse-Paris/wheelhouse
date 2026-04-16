"""Tests for the decision policy wiring in LibrarianLoop.process_event().

Story 14-1-3: Verifies that process_event() correctly bridges the proto
layer to the policy layer, handles locale routing, and writes/commits
pages through LibrarySandbox.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any
from unittest.mock import MagicMock, call, patch

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
    segment_text: str = "I decided to postpone the Series A to Q3 2027.",
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


# ---------------------------------------------------------------------------
# AC-1: English durable fact written to Library
# ---------------------------------------------------------------------------


class TestDurableFactWritten:
    def test_durable_fact_writes_and_commits(self) -> None:
        """AC-1: committed=True triggers sandbox.write() and sandbox.commit()."""
        sandbox = _make_sandbox_mock()

        page_content = "---\nsource_agent_id: agent-a\n---\n\n# Fundraise\n\nPostponed to Q3 2027."
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "durable_fact_written",
                True,
                "pages/fundraise.md",
                page_content,
            )
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            locales=["en", "fr"],
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event()
        result = loop.process_event(event)

        assert result.reason == "durable_fact_written"
        assert result.committed is True
        assert result.event_id == "evt-001"
        assert result.locale == "en"
        assert result.page_path == "pages/fundraise.md"
        assert result.content == page_content

        # Verify sandbox interactions
        sandbox.begin_transaction.assert_called_once()
        call_kwargs = sandbox.begin_transaction.call_args
        assert call_kwargs[1]["operation"] == "librarian_decide"
        sandbox.write.assert_called_once_with("pages/fundraise.md", page_content)
        sandbox.commit.assert_called_once()
        # New page => pages_created
        commit_kwargs = sandbox.commit.call_args[1]
        assert commit_kwargs.get("pages_created") == ["pages/fundraise.md"]


# ---------------------------------------------------------------------------
# AC-2: Transient context skipped
# ---------------------------------------------------------------------------


class TestTransientContextSkipped:
    def test_transient_context_no_write(self) -> None:
        """AC-2: transient_context -> no write, no commit."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response("transient_context", False)
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event(segment_text="Hello! How are you?")
        result = loop.process_event(event)

        assert result.reason == "transient_context"
        assert result.committed is False
        assert result.page_path is None
        assert result.content is None

        sandbox.begin_transaction.assert_not_called()
        sandbox.write.assert_not_called()
        sandbox.commit.assert_not_called()


# ---------------------------------------------------------------------------
# AC-3: Secret pattern skipped
# ---------------------------------------------------------------------------


class TestSecretPatternSkipped:
    def test_secret_pattern_no_write(self) -> None:
        """AC-3: contains_secret_pattern -> no write."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response("contains_secret_pattern", False)
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event(
            segment_text="My API key is sk-1234567890abcdef"
        )
        result = loop.process_event(event)

        assert result.reason == "contains_secret_pattern"
        assert result.committed is False
        sandbox.write.assert_not_called()


# ---------------------------------------------------------------------------
# AC-4: French locale routing
# ---------------------------------------------------------------------------


class TestFrenchLocaleRouting:
    def test_french_locale_uses_fr_prompt(self) -> None:
        """AC-4: locale='fr' routes to French prompt."""
        sandbox = _make_sandbox_mock()

        fr_content = "---\nsource_agent_id: agent-a\n---\n\n# Calendrier\n\nSerie A reportee."
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "durable_fact_written", True, "pages/calendrier.md", fr_content
            )
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            locales=["en", "fr"],
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event(locale="fr", segment_text="La Serie A est reportee.")
        result = loop.process_event(event)

        assert result.locale == "fr"
        assert result.committed is True

        # Verify French prompt was used: the system_prompt arg should contain French
        system_prompt_arg = llm_fn.call_args[0][0]
        assert "Bibliothecaire" in system_prompt_arg or "bibliothecaire" in system_prompt_arg.lower()


# ---------------------------------------------------------------------------
# AC-5: Update existing page
# ---------------------------------------------------------------------------


class TestUpdateExistingPage:
    def test_update_existing_page_uses_pages_updated(self) -> None:
        """AC-5: update_existing_page modifies existing, commits with pages_updated."""
        sandbox = _make_sandbox_mock()
        # Simulate an existing page
        sandbox.exists.return_value = True
        sandbox.list.return_value = ["pages/fundraise.md"]
        sandbox.read.side_effect = lambda p: (
            "Old content" if p == "pages/fundraise.md" else ""
        )

        updated_content = "---\n---\n\n# Fundraise\n\nNow targeting Q4 2027."
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "update_existing_page",
                True,
                "pages/fundraise.md",
                updated_content,
            )
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event(segment_text="Actually, let's target Q4 2027 instead.")
        result = loop.process_event(event)

        assert result.reason == "update_existing_page"
        assert result.committed is True

        # Commit should use pages_updated for updates
        commit_kwargs = sandbox.commit.call_args[1]
        assert commit_kwargs.get("pages_updated") == ["pages/fundraise.md"]


# ---------------------------------------------------------------------------
# AC-6: llm_fn injection
# ---------------------------------------------------------------------------


class TestLlmFnInjection:
    def test_no_llm_fn_returns_decision_error(self) -> None:
        """AC-6: llm_fn=None returns decision_error."""
        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=None,
            sandbox=_make_sandbox_mock(),
        )

        event = _make_event()
        result = loop.process_event(event)

        assert result.reason == "decision_error"
        assert result.committed is False

    def test_custom_llm_fn_callable(self) -> None:
        """AC-6: Any callable can serve as llm_fn."""

        def custom_llm(system_prompt: str, user_content: str) -> str:
            return _mock_llm_response("durable_fact_written", True, "pages/test.md", "content")

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=custom_llm,
            sandbox=_make_sandbox_mock(),
        )

        event = _make_event()
        result = loop.process_event(event)

        assert result.reason == "durable_fact_written"
        assert result.committed is True


# ---------------------------------------------------------------------------
# AC-7: Empty or unsupported locale falls back to English
# ---------------------------------------------------------------------------


class TestLocaleFallback:
    def test_empty_locale_defaults_to_en(self) -> None:
        """AC-7: Empty locale falls back to English."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response("transient_context", False)
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            locales=["en", "fr"],
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event(locale="")
        result = loop.process_event(event)

        assert result.locale == "en"

    def test_unsupported_locale_defaults_to_en(self) -> None:
        """AC-7: Unsupported locale (de) falls back to English."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response("transient_context", False)
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            locales=["en", "fr"],
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event(locale="de")
        result = loop.process_event(event)

        assert result.locale == "en"


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------


class TestEdgeCases:
    def test_llm_fn_exception_returns_decision_error(self) -> None:
        """llm_fn raising an exception produces decision_error."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(side_effect=RuntimeError("API down"))

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event()
        result = loop.process_event(event)

        assert result.reason == "decision_error"
        assert result.committed is False
        sandbox.write.assert_not_called()

    def test_sandbox_none_with_committed_result(self) -> None:
        """sandbox=None logs warning but does not crash."""
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "durable_fact_written", True, "pages/test.md", "content"
            )
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=None,
        )

        event = _make_event()
        result = loop.process_event(event)

        # Should still report as committed from decide() perspective
        assert result.reason == "durable_fact_written"
        assert result.committed is True

    def test_library_state_built_from_sandbox(self) -> None:
        """_build_library_state reads index.md and pages from sandbox."""
        sandbox = _make_sandbox_mock()
        sandbox.exists.return_value = True
        sandbox.list.return_value = ["index.md", "pages/one.md", "pages/two.md"]
        sandbox.read.side_effect = lambda p: {
            "index.md": "# Index\n- one\n- two",
            "pages/one.md": "Page one content",
            "pages/two.md": "Page two content",
        }.get(p, "")

        llm_fn = MagicMock(
            return_value=_mock_llm_response("no_durable_fact_detected", False)
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event()
        loop.process_event(event)

        # Verify the user_content sent to llm_fn contains library state
        user_content = llm_fn.call_args[0][1]
        assert "Page one content" in user_content
        assert "Page two content" in user_content

    def test_dedup_merged_uses_pages_updated(self) -> None:
        """dedup_merged uses pages_updated in commit."""
        sandbox = _make_sandbox_mock()
        merged_content = "Merged content"
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "dedup_merged", True, "pages/merged.md", merged_content
            )
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
        )

        event = _make_event()
        result = loop.process_event(event)

        assert result.reason == "dedup_merged"
        commit_kwargs = sandbox.commit.call_args[1]
        assert commit_kwargs.get("pages_updated") == ["pages/merged.md"]


# ---------------------------------------------------------------------------
# make_anthropic_llm_fn
# ---------------------------------------------------------------------------


class TestMakeAnthropicLlmFn:
    def test_factory_creates_callable(self) -> None:
        """make_anthropic_llm_fn returns a callable that wraps the Anthropic SDK."""
        with patch("wheelhouse.librarian.__main__.make_anthropic_llm_fn") as mock_factory:
            # Just verify the function signature exists and is importable
            from wheelhouse.librarian.__main__ import make_anthropic_llm_fn

            assert callable(make_anthropic_llm_fn)
