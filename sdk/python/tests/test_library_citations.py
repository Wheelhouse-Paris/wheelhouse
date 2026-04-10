"""Unit tests for library_citations (Story 13-15).

Covers AC-1..AC-8 from
``_bmad-output/implementation-artifacts/wh/13-15-citation-construction.md``.

Uses a real :class:`LibrarySandbox` over ``tmp_path`` for most tests
(no git init required — only ``.read()`` is exercised). AC-8 uses a
``MagicMock(spec=LibrarySandbox)`` so we can assert zero read calls on
an empty match list.
"""

from __future__ import annotations

import dataclasses
from unittest.mock import MagicMock

from wheelhouse.skills.library_citations import (
    Citation,
    UNKNOWN_SOURCE,
    build_citations,
    format_citations_inline,
    format_citations_markdown,
)
from wheelhouse.skills.library_retrieval import RetrievalMatch
from wheelhouse.skills.library_sandbox import LibrarySandbox


# ─── Helpers ──────────────────────────────────────────────────────────


def _page_with_front_matter(
    title: str,
    body: str,
    *,
    source: str | None = "brief.md",
    ingest_date: str | None = "2026-04-10T12:00:00Z",
) -> str:
    """Render a 13-8-style page for test fixtures.

    ``source`` / ``ingest_date`` set to ``None`` omits the key
    entirely, so AC-4 can exercise the missing-field fallback.
    """
    lines = ["---"]
    if source is not None:
        lines.append(f'source: "{source}"')
    if ingest_date is not None:
        lines.append(f"ingest_date: {ingest_date}")
    lines.append("cross_refs: []")
    lines.append("---")
    lines.append("")
    lines.append(f"# {title}")
    lines.append("")
    lines.append(body)
    return "\n".join(lines) + "\n"


def _make_sandbox(tmp_path) -> LibrarySandbox:
    """Build a real LibrarySandbox on a plain directory (no git)."""
    return LibrarySandbox(library_root=str(tmp_path), git_enabled=False)


# ─── AC-1: Citation dataclass shape ──────────────────────────────────


def test_ac1_citation_is_frozen_and_exposes_fields():
    c = Citation(
        page_slug="clients/acme.md",
        page_title="Acme",
        source="brief.md",
        ingest_date="2026-04-10T12:00:00Z",
        snippet="…",
    )
    assert c.page_slug == "clients/acme.md"
    assert c.page_title == "Acme"
    assert c.source == "brief.md"
    assert c.ingest_date == "2026-04-10T12:00:00Z"
    assert c.snippet == "…"
    # Frozen — assignment must fail.
    try:
        c.page_slug = "other.md"  # type: ignore[misc]
    except dataclasses.FrozenInstanceError:
        pass
    else:  # pragma: no cover
        raise AssertionError("Citation should be frozen")


# ─── AC-2: build_citations extracts source + ingest_date ─────────────


def test_ac2_build_citations_extracts_front_matter_fields(tmp_path):
    sandbox = _make_sandbox(tmp_path)
    (tmp_path / "clients").mkdir()
    sandbox.write(
        "clients/acme.md",
        _page_with_front_matter(
            "Acme Corp",
            "body text",
            source="brief.md",
            ingest_date="2026-04-10T12:00:00Z",
        ),
    )
    match = RetrievalMatch(
        slug="clients/acme.md",
        title="Acme Corp",
        snippet="…snippet…",
        score=7.0,
    )

    citations = build_citations([match], sandbox)

    assert len(citations) == 1
    c = citations[0]
    assert c.page_slug == "clients/acme.md"
    assert c.page_title == "Acme Corp"
    assert c.source == "brief.md"
    assert c.ingest_date == "2026-04-10T12:00:00Z"
    assert c.snippet == "…snippet…"


# ─── AC-3: preserves input order ─────────────────────────────────────


def test_ac3_build_citations_preserves_input_order(tmp_path):
    sandbox = _make_sandbox(tmp_path)
    slugs = ["one.md", "two.md", "three.md"]
    for i, slug in enumerate(slugs):
        sandbox.write(
            slug,
            _page_with_front_matter(
                f"Title {i}", "body", source=f"src-{i}.md"
            ),
        )
    matches = [
        RetrievalMatch(slug=slug, title=f"Title {i}", snippet="…", score=float(10 - i))
        for i, slug in enumerate(slugs)
    ]

    citations = build_citations(matches, sandbox)

    assert [c.page_slug for c in citations] == slugs
    assert [c.source for c in citations] == ["src-0.md", "src-1.md", "src-2.md"]


# ─── AC-4: tolerates missing `source` field ──────────────────────────


def test_ac4_build_citations_defaults_source_when_missing(tmp_path):
    sandbox = _make_sandbox(tmp_path)
    sandbox.write(
        "hand.md",
        _page_with_front_matter(
            "Hand-Authored",
            "body",
            source=None,  # omit key entirely
            ingest_date="2026-04-10",
        ),
    )
    match = RetrievalMatch(slug="hand.md", title="Hand-Authored", snippet="…", score=1.0)

    citations = build_citations([match], sandbox)

    assert len(citations) == 1
    assert citations[0].source == UNKNOWN_SOURCE
    assert citations[0].ingest_date == "2026-04-10"


def test_ac4_build_citations_defaults_ingest_date_when_missing(tmp_path):
    sandbox = _make_sandbox(tmp_path)
    sandbox.write(
        "hand.md",
        _page_with_front_matter(
            "Hand-Authored", "body", source="brief.md", ingest_date=None
        ),
    )
    match = RetrievalMatch(slug="hand.md", title="Hand-Authored", snippet="…", score=1.0)

    citations = build_citations([match], sandbox)

    assert len(citations) == 1
    assert citations[0].source == "brief.md"
    assert citations[0].ingest_date == ""


# ─── AC-5: silently drops a match whose page cannot be read ──────────


def test_ac5_build_citations_drops_unreadable_match(tmp_path):
    sandbox = _make_sandbox(tmp_path)
    match = RetrievalMatch(
        slug="nonexistent.md",
        title="Ghost",
        snippet="…",
        score=1.0,
    )

    citations = build_citations([match], sandbox)

    assert citations == []


def test_ac5_build_citations_drops_only_the_bad_match(tmp_path):
    sandbox = _make_sandbox(tmp_path)
    sandbox.write(
        "good.md",
        _page_with_front_matter("Good", "body", source="good-src.md"),
    )
    matches = [
        RetrievalMatch(slug="ghost.md", title="Ghost", snippet="…", score=2.0),
        RetrievalMatch(slug="good.md", title="Good", snippet="…", score=1.0),
    ]

    citations = build_citations(matches, sandbox)

    assert len(citations) == 1
    assert citations[0].page_slug == "good.md"
    assert citations[0].source == "good-src.md"


# ─── AC-6: format_citations_markdown footnote block ──────────────────


def test_ac6_format_citations_markdown_produces_numbered_lines():
    c1 = Citation(
        page_slug="a.md",
        page_title="Acme",
        source="brief.md",
        ingest_date="2026-04-10",
        snippet="…",
    )
    c2 = Citation(
        page_slug="p.md",
        page_title="Pricing",
        source="sheet.xlsx",
        ingest_date="2026-04-10",
        snippet="…",
    )

    out = format_citations_markdown([c1, c2])

    assert "[1] Acme (source: brief.md, ingested: 2026-04-10)" in out
    assert "[2] Pricing (source: sheet.xlsx, ingested: 2026-04-10)" in out
    # Lines are newline-separated, no trailing newline.
    assert out.count("\n") == 1
    assert not out.endswith("\n")


def test_ac6_format_citations_markdown_empty_list_returns_empty_string():
    assert format_citations_markdown([]) == ""


# ─── AC-7: format_citations_inline numbered markers ──────────────────


def test_ac7_format_citations_inline_numbers_markers():
    cs = [
        Citation(page_slug=f"p{i}.md", page_title=f"T{i}", source="s", ingest_date="d", snippet="…")
        for i in range(3)
    ]
    assert format_citations_inline(cs) == "[1] [2] [3]"


def test_ac7_format_citations_inline_empty_list_returns_empty_string():
    assert format_citations_inline([]) == ""


# ─── AC-8: empty match list skips sandbox reads ──────────────────────


def test_ac8_build_citations_empty_list_does_not_read_sandbox():
    mock_sandbox = MagicMock(spec=LibrarySandbox)

    result = build_citations([], mock_sandbox)

    assert result == []
    mock_sandbox.read.assert_not_called()
