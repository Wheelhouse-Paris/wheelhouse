"""Unit tests for the library_retrieval relevance-gated scan (Story 13-14).

Covers AC-1..AC-10 from
``_bmad-output/implementation-artifacts/wh/13-14-relevance-gated-retrieval.md``.

Tests mostly use a real :class:`LibrarySandbox` over ``tmp_path`` without
initializing git — :meth:`LibrarySandbox.list` and :meth:`LibrarySandbox.read`
only need a plain directory. The AC-10 "gate closed → zero I/O" test uses a
``MagicMock(spec=LibrarySandbox)`` so we can assert zero call counts.
"""

from __future__ import annotations

import dataclasses
import os
from typing import Any
from unittest.mock import MagicMock

import pytest

from wheelhouse.skills import library_retrieval as retr
from wheelhouse.skills.library_retrieval import (
    LibraryRetrievalResult,
    RetrievalMatch,
    SNIPPET_MAX_LEN,
    W_BODY,
    W_CROSS_REF,
    W_TITLE,
    relevance_gate,
    retrieve,
    search,
    tokenize,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox


# ─── Helpers ──────────────────────────────────────────────────────────


def _page_content(title: str, body: str, cross_refs: list[str] | None = None) -> str:
    """Render a page in the 13-8 front-matter layout for test fixtures."""
    refs = cross_refs or []
    refs_yaml = ", ".join(f'"{r}"' for r in refs)
    front = (
        "---\n"
        'source: "test"\n'
        "ingest_date: 2026-04-10\n"
        f"cross_refs: [{refs_yaml}]\n"
        "---\n"
        "\n"
        f"# {title}\n"
        f"{body}\n"
    )
    return front


def _make_sandbox(tmp_path, files: dict[str, str]) -> LibrarySandbox:
    """Write ``files`` into a ``tmp_path`` sandbox and return a LibrarySandbox."""
    for rel, content in files.items():
        abs_path = tmp_path / rel
        abs_path.parent.mkdir(parents=True, exist_ok=True)
        abs_path.write_text(content, encoding="utf-8")
    return LibrarySandbox(str(tmp_path))


# ─── AC-1: dataclass shapes ──────────────────────────────────────────


def test_retrieval_match_is_frozen_value_object() -> None:
    m = RetrievalMatch(
        slug="clients/acme.md",
        title="Acme",
        snippet="snippet",
        score=7.0,
    )
    assert m.slug == "clients/acme.md"
    assert m.title == "Acme"
    assert m.snippet == "snippet"
    assert m.score == 7.0

    with pytest.raises(dataclasses.FrozenInstanceError):
        m.slug = "other.md"  # type: ignore[misc]


def test_library_retrieval_result_shape() -> None:
    m = RetrievalMatch(slug="a.md", title="A", snippet="...", score=1.0)
    r = LibraryRetrievalResult(
        matches=[m],
        query="acme",
        total_pages_searched=3,
        elapsed_ms=4,
    )
    assert r.matches == [m]
    assert r.query == "acme"
    assert r.total_pages_searched == 3
    assert r.elapsed_ms == 4


# ─── AC-2: tokenizer ─────────────────────────────────────────────────


def test_tokenize_lowercases_splits_and_drops_stopwords() -> None:
    toks = tokenize("Acme Corp's PRICING plan (2026) and the pricing details")
    assert "acme" in toks
    assert "corp" in toks
    assert "pricing" in toks
    assert "plan" in toks
    assert "2026" in toks
    # stopwords gone
    assert "the" not in toks
    assert "and" not in toks
    assert "a" not in toks


def test_tokenize_ascii_folds_diacritics() -> None:
    toks = tokenize("Café München")
    assert "cafe" in toks
    assert "munchen" in toks


def test_tokenize_empty_string_returns_empty_list() -> None:
    assert tokenize("") == []
    assert tokenize("   ") == []


# ─── AC-3: title-weighted scoring ────────────────────────────────────


def test_search_ranks_title_match_above_body_only_match(tmp_path) -> None:
    sb = _make_sandbox(
        tmp_path,
        {
            "a.md": _page_content("Pricing", "the pricing tier is fair"),
            "b.md": _page_content(
                "Acme Corp",
                "they negotiated a pricing clause last year",
            ),
        },
    )
    result = search(sb, "pricing")
    assert len(result.matches) == 2
    assert result.matches[0].slug == "a.md"
    assert result.matches[1].slug == "b.md"
    assert result.matches[0].score > result.matches[1].score


# ─── AC-4: max_results ───────────────────────────────────────────────


def test_search_honors_max_results(tmp_path) -> None:
    files = {
        f"p{i}.md": _page_content(f"Page {i}", "foo is everywhere foo foo") for i in range(10)
    }
    sb = _make_sandbox(tmp_path, files)
    result = search(sb, "foo", max_results=3)
    assert len(result.matches) == 3
    assert result.total_pages_searched == 10


# ─── AC-5: snippet length cap ────────────────────────────────────────


def test_search_returns_snippet_at_most_200_chars_containing_token(tmp_path) -> None:
    filler = "lorem ipsum dolor sit amet " * 40  # > 1000 chars
    long_body = filler + " widget " + filler  # widget somewhere in the middle
    sb = _make_sandbox(
        tmp_path,
        {"long.md": _page_content("Long Page", long_body)},
    )
    result = search(sb, "widget")
    assert len(result.matches) == 1
    snippet = result.matches[0].snippet
    assert len(snippet) <= SNIPPET_MAX_LEN
    assert "widget" in snippet


# ─── AC-6: skip index.md and .git/* ──────────────────────────────────


def test_search_skips_index_md_and_dotgit(tmp_path) -> None:
    sb = _make_sandbox(
        tmp_path,
        {
            "index.md": _page_content("Index", "token lives here too"),
            "a.md": _page_content("Page A", "token appears on page A"),
            ".git/HEAD": "ref: refs/heads/main token token token",
        },
    )
    result = search(sb, "token")
    slugs = [m.slug for m in result.matches]
    assert slugs == ["a.md"]
    assert result.total_pages_searched == 1


# ─── AC-7: title > cross_refs > body weighting ───────────────────────


def test_search_weights_title_cross_refs_body(tmp_path) -> None:
    sb = _make_sandbox(
        tmp_path,
        {
            "t.md": _page_content("acme", "nothing relevant here"),
            "x.md": _page_content(
                "Other Topic",
                "unrelated body copy about weather",
                cross_refs=["acme.md"],
            ),
            "b.md": _page_content(
                "Other Topic Two",
                "one mention of acme in the body text",
            ),
        },
    )
    result = search(sb, "acme")
    slugs = [m.slug for m in result.matches]
    assert slugs == ["t.md", "x.md", "b.md"]

    scores = {m.slug: m.score for m in result.matches}
    assert scores["t.md"] > scores["x.md"] > scores["b.md"]
    # Sanity: constants are what we advertise.
    assert W_TITLE > W_CROSS_REF > W_BODY


# ─── AC-8: relevance_gate defaults to False ──────────────────────────


def test_relevance_gate_defaults_to_false_without_llm_fn() -> None:
    assert relevance_gate("tell me about acme pricing") is False
    assert relevance_gate("anything", domain_hint="sales") is False


# ─── AC-9: relevance_gate delegates to llm_fn ────────────────────────


def test_relevance_gate_delegates_to_injected_llm_fn_true() -> None:
    mock = MagicMock(return_value=True)
    out = relevance_gate(
        "tell me about acme pricing",
        domain_hint="sales crm",
        llm_fn=mock,
    )
    assert out is True
    mock.assert_called_once_with("tell me about acme pricing", "sales crm")


def test_relevance_gate_delegates_to_injected_llm_fn_false() -> None:
    mock = MagicMock(return_value=False)
    out = relevance_gate("what time is it?", llm_fn=mock)
    assert out is False
    mock.assert_called_once_with("what time is it?", None)


def test_relevance_gate_catches_llm_fn_exception_and_closes() -> None:
    def raiser(q: str, hint: str | None) -> bool:
        raise RuntimeError("LLM unavailable")

    assert relevance_gate("anything", llm_fn=raiser) is False


# ─── AC-10: retrieve skips scan when gate closes ─────────────────────


def test_retrieve_skips_scan_when_gate_closes() -> None:
    sb = MagicMock(spec=LibrarySandbox)
    # Make list/read blow up if accidentally called — the gate-closed
    # path MUST NOT touch the sandbox at all.
    sb.list.side_effect = AssertionError("list must not be called when gate is closed")
    sb.read.side_effect = AssertionError("read must not be called when gate is closed")

    gate_closed = MagicMock(return_value=False)
    result = retrieve(sb, "chit chat", llm_fn=gate_closed)

    assert result is None
    sb.list.assert_not_called()
    sb.read.assert_not_called()
    gate_closed.assert_called_once_with("chit chat", None)


def test_retrieve_runs_scan_when_gate_opens(tmp_path) -> None:
    sb = _make_sandbox(
        tmp_path,
        {
            "a.md": _page_content("Acme Corp", "body mentions acme several times"),
            "b.md": _page_content("Unrelated", "weather report"),
        },
    )
    gate_open = MagicMock(return_value=True)
    result = retrieve(sb, "acme", llm_fn=gate_open)

    assert result is not None
    assert isinstance(result, LibraryRetrievalResult)
    assert result.query == "acme"
    assert len(result.matches) >= 1
    assert result.matches[0].slug == "a.md"
    gate_open.assert_called_once_with("acme", None)


def test_retrieve_none_with_no_llm_fn(tmp_path) -> None:
    """Safe-default: no llm_fn → gate closed → no retrieval."""
    sb = _make_sandbox(
        tmp_path,
        {"a.md": _page_content("Acme", "body about acme")},
    )
    result = retrieve(sb, "acme")
    assert result is None


# ─── Additional coverage ─────────────────────────────────────────────


def test_search_records_elapsed_ms_and_total_pages(tmp_path) -> None:
    sb = _make_sandbox(
        tmp_path,
        {
            "a.md": _page_content("A", "token present"),
            "b.md": _page_content("B", "nothing here"),
        },
    )
    result = search(sb, "token")
    assert result.total_pages_searched == 2
    assert result.elapsed_ms >= 0


def test_search_falls_back_to_slug_title_when_no_h1(tmp_path) -> None:
    raw = (
        "---\n"
        'source: "test"\n'
        "ingest_date: 2026-04-10\n"
        "cross_refs: []\n"
        "---\n"
        "\n"
        "plain body with token and nothing else\n"
    )
    sb = _make_sandbox(tmp_path, {"headless.md": raw})
    result = search(sb, "token")
    assert len(result.matches) == 1
    assert result.matches[0].title == "headless.md"


def test_search_zero_score_pages_excluded(tmp_path) -> None:
    sb = _make_sandbox(
        tmp_path,
        {
            "a.md": _page_content("Acme", "acme acme acme"),
            "b.md": _page_content("Other", "weather report"),
        },
    )
    result = search(sb, "acme")
    slugs = [m.slug for m in result.matches]
    assert slugs == ["a.md"]
    # b.md was scanned but not returned because score == 0
    assert result.total_pages_searched == 2
