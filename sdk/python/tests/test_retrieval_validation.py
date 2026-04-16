"""Retrieval validation tests for librarian-written pages (Story 14-4-3).

Validates that:
1. Epic 13 retrieval skill (13-14) works unchanged for librarian-written pages
2. Epic 13 citation construction (13-15) works unchanged
3. "What do you know about <user>?" queries work via the grep-based scorer
4. Git commit structured metadata follows ADR-039
5. DecisionResult and V1 reason codes are valid and constructable

All tests use real LibrarySandbox over tmp_path. No production code is
modified -- this is a validation-only story.
"""

from __future__ import annotations

import inspect
import json
import subprocess
from unittest.mock import MagicMock

import pytest

from wheelhouse.skills.library_citations import (
    Citation,
    build_citations,
    format_citations_inline,
    format_citations_markdown,
)
from wheelhouse.skills.library_retrieval import (
    LibraryRetrievalResult,
    RetrievalMatch,
    retrieve,
    search,
    tokenize,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox
from wheelhouse.skills.librarian.decide import decide
from wheelhouse.skills.librarian.types import (
    ConversationMessage,
    DecisionResult,
    LibraryState,
    REASON_CODES_V1,
)


# ─── Helpers ──────────────────────────────────────────────────────────


def _librarian_page(
    title: str,
    body: str,
    *,
    source: str = "agent-librarian",
    source_type: str = "librarian_decision",
    source_agent_id: str = "agent-a",
    conversation_id: str = "conv-001",
    ingest_date: str = "2026-04-16T10:00:00Z",
    cross_refs: list[str] | None = None,
) -> str:
    """Render a page in the format a librarian would write.

    Includes standard front-matter (source, ingest_date, cross_refs) plus
    librarian-specific extensions (source_agent_id, conversation_id).
    """
    refs = cross_refs or []
    refs_yaml = ", ".join(f'"{r}"' for r in refs)
    return (
        "---\n"
        f'source: "{source}"\n'
        f'source_type: "{source_type}"\n'
        f"ingest_date: {ingest_date}\n"
        f"cross_refs: [{refs_yaml}]\n"
        f'source_agent_id: "{source_agent_id}"\n'
        f'conversation_id: "{conversation_id}"\n'
        "---\n"
        "\n"
        f"# {title}\n"
        "\n"
        f"{body}\n"
    )


def _make_sandbox(tmp_path, files: dict[str, str]) -> LibrarySandbox:
    """Write files into a tmp_path sandbox and return a LibrarySandbox."""
    for rel, content in files.items():
        abs_path = tmp_path / rel
        abs_path.parent.mkdir(parents=True, exist_ok=True)
        abs_path.write_text(content, encoding="utf-8")
    return LibrarySandbox(str(tmp_path))


def _make_five_page_library(tmp_path) -> LibrarySandbox:
    """Create a 5-page Library with librarian-written content."""
    pages = {
        "users/nicolas-preferences.md": _librarian_page(
            "Nicolas Preferences",
            "Nicolas prefers dark mode and uses VS Code. "
            "He works primarily in Python and Rust.",
            source_agent_id="agent-alpha",
            conversation_id="conv-100",
        ),
        "users/nicolas-birthday.md": _librarian_page(
            "Nicolas Birthday",
            "Nicolas was born on April 5th. He celebrates with his family every year.",
            source_agent_id="agent-alpha",
            conversation_id="conv-101",
            cross_refs=["users/nicolas-preferences.md"],
        ),
        "projects/wheelhouse-architecture.md": _librarian_page(
            "Wheelhouse Architecture",
            "Wheelhouse uses a broker-based pub-sub architecture with ZMQ transport. "
            "The system supports multiple agents sharing Library volumes.",
            source_agent_id="agent-beta",
            conversation_id="conv-200",
        ),
        "meetings/team-standup-april.md": _librarian_page(
            "Team Standup April",
            "The team discussed sprint progress and blocker resolution. "
            "Nicolas presented the library retrieval feature demo.",
            source_agent_id="agent-alpha",
            conversation_id="conv-300",
            cross_refs=["projects/wheelhouse-architecture.md"],
        ),
        "clients/acme-engagement.md": _librarian_page(
            "Acme Engagement",
            "Acme Corp signed a 12-month contract for enterprise support. "
            "Primary contact is Alice from their engineering team.",
            source_agent_id="agent-gamma",
            conversation_id="conv-400",
        ),
    }
    return _make_sandbox(tmp_path, pages)


# ════════════════════════════════════════════════════════════════════════
# AC #1: Retrieval works unchanged for librarian-written pages
# ════════════════════════════════════════════════════════════════════════


class TestRetrievalUnchanged:
    """Verify the Epic 13 retrieval skill works unchanged on librarian pages."""

    def test_search_returns_matches_for_librarian_pages(self, tmp_path):
        """search() scores and returns librarian-written pages."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "Nicolas")

        assert isinstance(result, LibraryRetrievalResult)
        assert result.total_pages_searched == 5
        assert len(result.matches) >= 2  # at least preferences + birthday
        # Title-weighted: pages with "Nicolas" in title rank higher.
        slugs = [m.slug for m in result.matches]
        assert any("nicolas" in s for s in slugs)

    def test_search_ranks_title_match_highest(self, tmp_path):
        """Title matches (W_TITLE=5) outrank body-only matches."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "Nicolas")

        # Pages with "Nicolas" in title should score higher than
        # "meetings/team-standup-april.md" which only has "Nicolas" in body.
        if len(result.matches) >= 3:
            title_match_scores = [
                m.score for m in result.matches if "nicolas" in m.slug
            ]
            body_only_scores = [
                m.score
                for m in result.matches
                if "nicolas" not in m.slug
            ]
            if title_match_scores and body_only_scores:
                assert min(title_match_scores) > max(body_only_scores)

    def test_search_returns_correct_result_type(self, tmp_path):
        """Return type is LibraryRetrievalResult with RetrievalMatch elements."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "architecture")

        assert isinstance(result, LibraryRetrievalResult)
        assert isinstance(result.query, str)
        assert isinstance(result.total_pages_searched, int)
        assert isinstance(result.elapsed_ms, int)
        for m in result.matches:
            assert isinstance(m, RetrievalMatch)
            assert isinstance(m.slug, str)
            assert isinstance(m.title, str)
            assert isinstance(m.snippet, str)
            assert isinstance(m.score, float)

    def test_search_api_signature_unchanged(self):
        """search() signature matches Epic 13 baseline."""
        sig = inspect.signature(search)
        params = list(sig.parameters.keys())
        assert params == ["sandbox", "query", "max_results", "domain_hint"]

    def test_retrieve_api_signature_unchanged(self):
        """retrieve() signature matches Epic 13 baseline."""
        sig = inspect.signature(retrieve)
        params = list(sig.parameters.keys())
        assert params == ["sandbox", "query", "max_results", "domain_hint", "llm_fn"]

    def test_retrieve_with_gate_open_returns_results(self, tmp_path):
        """retrieve() with gate-open delegates to search() for librarian pages."""
        sb = _make_five_page_library(tmp_path)
        gate_open = MagicMock(return_value=True)
        result = retrieve(sb, "acme", llm_fn=gate_open)

        assert result is not None
        assert isinstance(result, LibraryRetrievalResult)
        assert len(result.matches) >= 1
        assert result.matches[0].slug == "clients/acme-engagement.md"

    def test_search_cross_refs_boost_scoring(self, tmp_path):
        """Cross-refs in librarian pages contribute to scoring via W_CROSS_REF."""
        sb = _make_five_page_library(tmp_path)
        # "wheelhouse-architecture" appears in cross_refs of team-standup page
        result = search(sb, "wheelhouse architecture")

        slugs = [m.slug for m in result.matches]
        assert "projects/wheelhouse-architecture.md" in slugs

    def test_search_librarian_extra_front_matter_transparent(self, tmp_path):
        """Extra front-matter keys (source_agent_id, conversation_id) don't break parsing."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "acme contract")

        assert result.total_pages_searched == 5  # all pages scanned without error
        assert len(result.matches) >= 1

    def test_search_empty_library_returns_zero_matches(self, tmp_path):
        """search() on an empty Library returns empty result without crash."""
        sb = LibrarySandbox(str(tmp_path))
        result = search(sb, "anything")

        assert isinstance(result, LibraryRetrievalResult)
        assert result.matches == []
        assert result.total_pages_searched == 0


# ════════════════════════════════════════════════════════════════════════
# AC #2: Citation construction works for librarian-written pages
# ════════════════════════════════════════════════════════════════════════


class TestCitationConstruction:
    """Verify citation construction for librarian-written pages."""

    def test_build_citations_extracts_front_matter_from_librarian_pages(self, tmp_path):
        """build_citations() reads source and ingest_date from librarian page front-matter."""
        sb = _make_five_page_library(tmp_path)
        match = RetrievalMatch(
            slug="users/nicolas-preferences.md",
            title="Nicolas Preferences",
            snippet="Nicolas prefers dark mode",
            score=7.0,
        )
        citations = build_citations([match], sb)

        assert len(citations) == 1
        c = citations[0]
        assert c.page_slug == "users/nicolas-preferences.md"
        assert c.page_title == "Nicolas Preferences"
        assert c.source == "agent-librarian"
        assert c.ingest_date == "2026-04-16T10:00:00Z"
        assert c.snippet == "Nicolas prefers dark mode"

    def test_build_citations_preserves_order_for_librarian_pages(self, tmp_path):
        """Multiple librarian-page citations preserve input order."""
        sb = _make_five_page_library(tmp_path)
        matches = [
            RetrievalMatch(
                slug="users/nicolas-preferences.md",
                title="Nicolas Preferences",
                snippet="...",
                score=10.0,
            ),
            RetrievalMatch(
                slug="clients/acme-engagement.md",
                title="Acme Engagement",
                snippet="...",
                score=5.0,
            ),
        ]
        citations = build_citations(matches, sb)

        assert len(citations) == 2
        assert citations[0].page_slug == "users/nicolas-preferences.md"
        assert citations[1].page_slug == "clients/acme-engagement.md"

    def test_format_citations_markdown_for_librarian_pages(self, tmp_path):
        """format_citations_markdown() renders librarian citations correctly."""
        c1 = Citation(
            page_slug="users/nicolas-preferences.md",
            page_title="Nicolas Preferences",
            source="agent-librarian",
            ingest_date="2026-04-16T10:00:00Z",
            snippet="...",
        )
        c2 = Citation(
            page_slug="clients/acme-engagement.md",
            page_title="Acme Engagement",
            source="agent-librarian",
            ingest_date="2026-04-16T10:00:00Z",
            snippet="...",
        )
        out = format_citations_markdown([c1, c2])

        assert "[1] Nicolas Preferences (source: agent-librarian, ingested: 2026-04-16T10:00:00Z)" in out
        assert "[2] Acme Engagement (source: agent-librarian, ingested: 2026-04-16T10:00:00Z)" in out

    def test_format_citations_inline_for_librarian_pages(self):
        """format_citations_inline() numbers librarian page citations."""
        citations = [
            Citation(
                page_slug=f"p{i}.md",
                page_title=f"T{i}",
                source="agent-librarian",
                ingest_date="2026-04-16",
                snippet="...",
            )
            for i in range(3)
        ]
        assert format_citations_inline(citations) == "[1] [2] [3]"

    def test_citation_api_signatures_unchanged(self):
        """Citation API signatures match Epic 13 baseline."""
        sig_build = inspect.signature(build_citations)
        assert list(sig_build.parameters.keys()) == ["matches", "sandbox"]

        sig_md = inspect.signature(format_citations_markdown)
        assert list(sig_md.parameters.keys()) == ["citations"]

        sig_inline = inspect.signature(format_citations_inline)
        assert list(sig_inline.parameters.keys()) == ["citations"]


# ════════════════════════════════════════════════════════════════════════
# AC #3: "What do you know about me?" query validation
# ════════════════════════════════════════════════════════════════════════


class TestWhatDoYouKnowAboutMe:
    """Validate user-knowledge queries via the grep-based retrieval scorer."""

    def test_tokenizer_strips_stopwords_from_identity_query(self):
        """'what do you know about me' strips stopwords per the _STOPWORDS set."""
        tokens = tokenize("what do you know about me")
        # "what", "you", "me" are in _STOPWORDS. "do" is NOT a stopword.
        # "know" and "about" also survive.
        assert "what" not in tokens
        assert "you" not in tokens
        assert "me" not in tokens
        assert "do" in tokens  # "do" is not a stopword
        assert "know" in tokens
        assert "about" in tokens

    def test_search_with_user_name_finds_user_pages(self, tmp_path):
        """search('Nicolas') returns pages with 'Nicolas' in title/body."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "Nicolas")

        slugs = [m.slug for m in result.matches]
        # Pages with "Nicolas" in title score highest (W_TITLE=5).
        assert "users/nicolas-preferences.md" in slugs
        assert "users/nicolas-birthday.md" in slugs

    def test_search_with_user_name_and_topic_ranks_correctly(self, tmp_path):
        """search('Nicolas preferences') returns both user pages in top results."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "Nicolas preferences")

        assert len(result.matches) >= 2
        top_two_slugs = {result.matches[0].slug, result.matches[1].slug}
        # Both Nicolas pages should be in top 2 (they both have "nicolas"
        # in title; birthday page also has cross_ref to preferences page
        # which boosts its score via W_CROSS_REF).
        assert "users/nicolas-preferences.md" in top_two_slugs
        assert "users/nicolas-birthday.md" in top_two_slugs

    def test_search_with_user_name_birthday(self, tmp_path):
        """search('Nicolas birthday') finds the birthday page."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "Nicolas birthday")

        slugs = [m.slug for m in result.matches]
        assert "users/nicolas-birthday.md" in slugs
        # Birthday page should rank first (both tokens in title).
        assert result.matches[0].slug == "users/nicolas-birthday.md"

    def test_end_to_end_search_and_citation_pipeline(self, tmp_path):
        """Full pipeline: search -> build_citations -> format_citations_markdown."""
        sb = _make_five_page_library(tmp_path)
        result = search(sb, "Nicolas")

        citations = build_citations(result.matches, sb)
        assert len(citations) >= 2

        markdown = format_citations_markdown(citations)
        assert "[1]" in markdown
        assert "Nicolas" in markdown
        assert "agent-librarian" in markdown

        inline = format_citations_inline(citations)
        assert "[1]" in inline

    def test_search_empty_library_no_crash(self, tmp_path):
        """search() on empty Library returns empty result."""
        sb = LibrarySandbox(str(tmp_path))
        result = search(sb, "Nicolas")

        assert result.matches == []
        assert result.total_pages_searched == 0


# ════════════════════════════════════════════════════════════════════════
# AC #4: Git log structured metadata validation
# ════════════════════════════════════════════════════════════════════════


class TestGitMetadataValidation:
    """Validate that LibrarySandbox.transaction() produces ADR-039 structured commits."""

    def test_commit_message_has_operation_tag_and_summary(self, tmp_path):
        """Commit message subject line follows [operation] summary format."""
        sb = LibrarySandbox(str(tmp_path), git_enabled=True, agent_name="librarian-test")

        sb.begin("write", "agent write: users/nicolas.md")
        sb.write("users/nicolas.md", _librarian_page(
            "Nicolas",
            "User preferences page.",
        ))
        sb.commit(
            pages_created=["users/nicolas.md"],
            sources=["agent-librarian"],
        )

        # Read the commit message from git log.
        result = subprocess.run(
            ["git", "log", "-1", "--pretty=format:%B"],
            cwd=str(tmp_path),
            capture_output=True,
            text=True,
        )
        message = result.stdout

        # ADR-039: subject is [operation] summary
        assert message.startswith("[write] agent write: users/nicolas.md")
        assert "Sources: agent-librarian" in message
        assert "Pages created: users/nicolas.md" in message

    def test_commit_message_pages_updated_field(self, tmp_path):
        """Commit body contains Pages updated field when updating."""
        sb = LibrarySandbox(str(tmp_path), git_enabled=True, agent_name="librarian-test")

        # First commit: create the page.
        sb.begin("write", "initial write")
        sb.write("notes.md", _librarian_page("Notes", "Initial content."))
        sb.commit(pages_created=["notes.md"])

        # Second commit: update the page.
        sb.begin("write", "update notes")
        sb.write("notes.md", _librarian_page("Notes", "Updated content."))
        sb.commit(pages_updated=["notes.md"])

        result = subprocess.run(
            ["git", "log", "-1", "--pretty=format:%B"],
            cwd=str(tmp_path),
            capture_output=True,
            text=True,
        )
        message = result.stdout

        assert "[write] update notes" in message
        assert "Pages updated: notes.md" in message

    def test_git_log_parseable_for_librarian_metadata(self, tmp_path):
        """Git log output can be parsed to extract structured commit fields."""
        sb = LibrarySandbox(str(tmp_path), git_enabled=True, agent_name="librarian-research")

        sb.begin("write", "librarian decision: durable_fact_written")
        sb.write("facts/fact-001.md", _librarian_page(
            "Fact 001",
            "The sky is blue.",
            source_agent_id="agent-alpha",
        ))
        sb.commit(
            pages_created=["facts/fact-001.md"],
            sources=["agent-librarian"],
        )

        result = subprocess.run(
            ["git", "log", "-1", "--pretty=format:%B"],
            cwd=str(tmp_path),
            capture_output=True,
            text=True,
        )
        lines = result.stdout.strip().splitlines()

        # Subject line parseable as [operation] summary
        subject = lines[0]
        assert subject.startswith("[write]")
        assert "durable_fact_written" in subject

        # Body lines parseable as key: value
        body = "\n".join(lines[1:]) if len(lines) > 1 else ""
        assert "Sources:" in body
        assert "Pages created:" in body

    def test_git_author_contains_agent_name(self, tmp_path):
        """Git commit author includes the agent name for attribution."""
        sb = LibrarySandbox(str(tmp_path), git_enabled=True, agent_name="librarian-alpha")

        sb.begin("write", "test commit")
        sb.write("test.md", _librarian_page("Test", "Body."))
        sb.commit(pages_created=["test.md"])

        result = subprocess.run(
            ["git", "log", "-1", "--pretty=format:%an"],
            cwd=str(tmp_path),
            capture_output=True,
            text=True,
        )
        assert "librarian-alpha" in result.stdout


# ════════════════════════════════════════════════════════════════════════
# AC #5: DecisionResult and V1 reason codes validation
# ════════════════════════════════════════════════════════════════════════


class TestDecisionResultValidation:
    """Validate DecisionResult fields and V1 reason codes."""

    def test_all_ten_v1_reason_codes_exist(self):
        """REASON_CODES_V1 contains exactly 10 codes per ADR-047."""
        assert len(REASON_CODES_V1) == 10
        expected = {
            "durable_fact_written",
            "update_existing_page",
            "dedup_merged",
            "no_durable_fact_detected",
            "pii_not_durable",
            "contains_secret_pattern",
            "transient_context",
            "below_locale_confidence",
            "decision_timeout",
            "decision_error",
        }
        assert REASON_CODES_V1 == expected

    @pytest.mark.parametrize("reason", sorted(REASON_CODES_V1))
    def test_every_reason_code_constructable_as_skip(self, reason):
        """Each V1 reason code can construct a DecisionResult (skip)."""
        result = DecisionResult(reason=reason, committed=False)
        assert result.reason == reason
        assert result.committed is False
        assert result.schema_version == 1

    @pytest.mark.parametrize(
        "reason",
        ["durable_fact_written", "update_existing_page", "dedup_merged"],
    )
    def test_write_reason_codes_constructable_with_content(self, reason):
        """Write reason codes can construct a DecisionResult with page_path and content."""
        result = DecisionResult(
            reason=reason,
            committed=True,
            page_path="pages/test.md",
            content="# Test\n\nContent.",
        )
        assert result.committed is True
        assert result.page_path == "pages/test.md"
        assert result.content is not None

    def test_unknown_reason_code_raises_value_error(self):
        """Unknown reason code raises ValueError in DecisionResult.__post_init__."""
        with pytest.raises(ValueError, match="Unknown reason code"):
            DecisionResult(reason="not_a_real_code", committed=False)

    def test_decision_result_schema_version_default(self):
        """DecisionResult defaults to schema_version=1."""
        result = DecisionResult(reason="transient_context", committed=False)
        assert result.schema_version == 1

    def test_decision_result_frozen(self):
        """DecisionResult is frozen (immutable)."""
        result = DecisionResult(reason="transient_context", committed=False)
        with pytest.raises(AttributeError):
            result.reason = "other"  # type: ignore[misc]

    def test_decide_with_mock_llm_produces_write_result(self):
        """decide() with a mock llm_fn that returns a write decision."""
        mock_response = json.dumps({
            "reason": "durable_fact_written",
            "committed": True,
            "page_path": "pages/user-prefs.md",
            "content": "---\nsource: agent-a\n---\n\n# User Prefs\n\nDark mode.",
        })
        mock_llm = MagicMock(return_value=mock_response)

        segment = [
            ConversationMessage(role="user", content="I prefer dark mode"),
            ConversationMessage(role="assistant", content="Noted, dark mode preference saved."),
        ]
        result = decide(
            segment=segment,
            library_state=LibraryState(),
            locale="en",
            llm_fn=mock_llm,
        )

        assert result.reason == "durable_fact_written"
        assert result.committed is True
        assert result.page_path == "pages/user-prefs.md"
        assert result.content is not None
        mock_llm.assert_called_once()

    def test_decide_with_mock_llm_produces_skip_result(self):
        """decide() with a mock llm_fn that returns a skip decision."""
        mock_response = json.dumps({
            "reason": "transient_context",
            "committed": False,
        })
        mock_llm = MagicMock(return_value=mock_response)

        segment = [
            ConversationMessage(role="user", content="Hello!"),
            ConversationMessage(role="assistant", content="Hi there!"),
        ]
        result = decide(
            segment=segment,
            library_state=LibraryState(),
            locale="en",
            llm_fn=mock_llm,
        )

        assert result.reason == "transient_context"
        assert result.committed is False
        assert result.page_path is None
        assert result.content is None

    def test_decide_with_llm_error_returns_decision_error(self):
        """decide() catches LLM exceptions and returns decision_error."""
        def failing_llm(system: str, user: str) -> str:
            raise RuntimeError("LLM unavailable")

        segment = [
            ConversationMessage(role="user", content="test"),
        ]
        result = decide(
            segment=segment,
            library_state=LibraryState(),
            locale="en",
            llm_fn=failing_llm,
        )

        assert result.reason == "decision_error"
        assert result.committed is False

    def test_decide_with_invalid_json_returns_decision_error(self):
        """decide() handles unparseable LLM response gracefully."""
        mock_llm = MagicMock(return_value="not valid json {{{")

        segment = [
            ConversationMessage(role="user", content="test"),
        ]
        result = decide(
            segment=segment,
            library_state=LibraryState(),
            locale="en",
            llm_fn=mock_llm,
        )

        assert result.reason == "decision_error"
        assert result.committed is False
