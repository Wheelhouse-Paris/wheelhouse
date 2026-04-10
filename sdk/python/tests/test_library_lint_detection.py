"""Unit tests for the library_lint detection pipeline (Story 13-16).

Covers AC-1..AC-9 from
``_bmad-output/implementation-artifacts/wh/13-16-lint-detection-pipeline.md``.

Every test runs in memory: the LibrarySandbox is a MagicMock speccing
``LibrarySandbox``, and the LLM detector is a plain callable stub. No
real git, no real filesystem, no real LLM.
"""

from __future__ import annotations

from typing import Sequence
from unittest.mock import MagicMock

import pytest

from wheelhouse.skills import library_lint as lint_mod
from wheelhouse.skills.library_lint import (
    CATEGORY_CONTRADICTION,
    CATEGORY_ORPHAN,
    CATEGORY_OUTDATED_CLAIM,
    CATEGORY_STALE_REF,
    LibraryPage,
    LintFinding,
    SEVERITY_ERROR,
    SEVERITY_INFO,
    SEVERITY_WARN,
    detect_contradictions,
    detect_orphans,
    detect_outdated_claims,
    detect_stale_refs,
    lint_library,
    parse_pages,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox


# ─── Helpers ──────────────────────────────────────────────────────────


def _page(path: str, refs: Sequence[str] = (), body: str = "") -> LibraryPage:
    return LibraryPage(
        path=path,
        body=body,
        cross_refs=tuple(refs),
        front_matter={"cross_refs": list(refs)},
    )


def _make_sandbox(files: dict[str, str]) -> MagicMock:
    """Build a MagicMock sandbox whose .list() and .read() drive the fixture."""
    sb = MagicMock(spec=LibrarySandbox)
    sb.list.return_value = sorted(files.keys())

    def _read(path: str) -> str:
        return files[path]

    sb.read.side_effect = _read
    return sb


# ─── AC-1: LintFinding shape ──────────────────────────────────────────


def test_lint_finding_is_a_frozen_value_object() -> None:
    f = LintFinding(
        category=CATEGORY_ORPHAN,
        page="clients/acme.md",
        detail="no inbound refs",
        severity=SEVERITY_WARN,
    )
    assert f.category == CATEGORY_ORPHAN
    assert f.page == "clients/acme.md"
    assert f.detail == "no inbound refs"
    assert f.severity == SEVERITY_WARN

    import dataclasses

    with pytest.raises(dataclasses.FrozenInstanceError):
        f.page = "other.md"  # type: ignore[misc]


# ─── AC-2: orphan detection ───────────────────────────────────────────


def test_detect_orphans_flags_pages_without_inbound_refs() -> None:
    pages = [
        _page("index.md", refs=["topics/pricing.md"]),
        _page("topics/pricing.md", refs=[]),
        _page("clients/acme.md", refs=[]),  # no inbound → orphan
    ]
    findings = detect_orphans(pages)
    assert len(findings) == 1
    assert findings[0].category == CATEGORY_ORPHAN
    assert findings[0].page == "clients/acme.md"
    assert findings[0].severity == SEVERITY_WARN


def test_detect_orphans_never_flags_index_md() -> None:
    pages = [
        _page("index.md", refs=[]),  # no inbound — but index.md is exempt
        _page("topics/pricing.md", refs=[]),  # orphan
    ]
    findings = detect_orphans(pages)
    assert [f.page for f in findings] == ["topics/pricing.md"]


# ─── AC-3: stale-ref detection ────────────────────────────────────────


def test_detect_stale_refs_flags_missing_targets() -> None:
    pages = [
        _page("A.md", refs=["B.md", "gone.md"]),
        _page("B.md", refs=[]),
    ]
    findings = detect_stale_refs(pages)
    assert len(findings) == 1
    assert findings[0].category == CATEGORY_STALE_REF
    assert findings[0].page == "A.md"
    assert "gone.md" in findings[0].detail


def test_detect_stale_refs_clean_library_has_no_findings() -> None:
    pages = [
        _page("A.md", refs=["B.md"]),
        _page("B.md", refs=["A.md"]),
    ]
    assert detect_stale_refs(pages) == []


# ─── AC-4: contradictions via injected llm_fn ─────────────────────────


def test_detect_contradictions_lifts_llm_verdicts_into_findings() -> None:
    pages = [_page("a.md"), _page("b.md")]
    llm_fn = MagicMock(
        return_value=[
            {"pages": ["a.md", "b.md"], "detail": "price differs: $10 vs $12"}
        ]
    )

    findings = detect_contradictions(pages, llm_fn)

    assert len(findings) == 1
    assert findings[0].category == CATEGORY_CONTRADICTION
    assert findings[0].page == "a.md"
    assert "price" in findings[0].detail
    assert findings[0].severity == SEVERITY_ERROR
    llm_fn.assert_called_once()
    # Second positional arg is the detector name.
    assert llm_fn.call_args.args[1] == CATEGORY_CONTRADICTION


def test_detect_contradictions_tolerates_empty_or_malformed_verdicts() -> None:
    pages = [_page("a.md")]
    llm_fn = MagicMock(
        return_value=[
            {},  # no pages key
            {"pages": []},  # empty pages list
            "not a dict",  # wrong type
            {"pages": ["a.md"], "detail": "solo claim"},
        ]
    )
    findings = detect_contradictions(pages, llm_fn)
    assert len(findings) == 1
    assert findings[0].page == "a.md"


# ─── AC-5: outdated claims via injected llm_fn ────────────────────────


def test_detect_outdated_claims_lifts_llm_verdicts_into_findings() -> None:
    pages = [_page("news.md")]
    llm_fn = MagicMock(
        return_value=[{"page": "news.md", "detail": "as of 2024 ..."}]
    )

    findings = detect_outdated_claims(pages, llm_fn)

    assert len(findings) == 1
    assert findings[0].category == CATEGORY_OUTDATED_CLAIM
    assert findings[0].page == "news.md"
    assert findings[0].severity == SEVERITY_INFO
    llm_fn.assert_called_once()
    assert llm_fn.call_args.args[1] == CATEGORY_OUTDATED_CLAIM


# ─── AC-6: orchestrator merges findings ───────────────────────────────


def test_lint_library_merges_all_detectors() -> None:
    files = {
        "index.md": "---\ncross_refs: [a.md]\n---\n# Index\n",
        "a.md": "---\ncross_refs: [gone.md]\n---\n# A\n",
        "orphan.md": "---\ncross_refs: []\n---\n# Orphan\n",
    }
    sandbox = _make_sandbox(files)
    llm_fn = MagicMock(return_value=[])

    findings = lint_library(sandbox, llm_fn=llm_fn)

    categories = {f.category for f in findings}
    assert CATEGORY_ORPHAN in categories
    assert CATEGORY_STALE_REF in categories
    # llm_fn should have been invoked twice: once per LLM-backed detector.
    assert llm_fn.call_count == 2
    seen_detectors = {call.args[1] for call in llm_fn.call_args_list}
    assert seen_detectors == {CATEGORY_CONTRADICTION, CATEGORY_OUTDATED_CLAIM}

    # Orphan must be "orphan.md" (index.md is exempt).
    orphan_pages = {f.page for f in findings if f.category == CATEGORY_ORPHAN}
    assert orphan_pages == {"orphan.md"}

    # Stale ref must flag "gone.md" from a.md.
    stale = [f for f in findings if f.category == CATEGORY_STALE_REF]
    assert len(stale) == 1
    assert stale[0].page == "a.md"
    assert "gone.md" in stale[0].detail


# ─── AC-7: llm_fn=None skips LLM detectors gracefully ─────────────────


def test_lint_library_without_llm_fn_runs_only_pure_detectors() -> None:
    files = {
        "index.md": "---\ncross_refs: [a.md]\n---\n",
        "a.md": "---\ncross_refs: [gone.md]\n---\n",
        "b.md": "---\ncross_refs: []\n---\n",
    }
    sandbox = _make_sandbox(files)

    findings = lint_library(sandbox, llm_fn=None)

    categories = {f.category for f in findings}
    # Pure detectors only.
    assert CATEGORY_ORPHAN in categories
    assert CATEGORY_STALE_REF in categories
    assert CATEGORY_CONTRADICTION not in categories
    assert CATEGORY_OUTDATED_CLAIM not in categories


# ─── AC-8: page_filter restricts scanning ─────────────────────────────


def test_lint_library_page_filter_skips_unlisted_pages() -> None:
    files = {
        "a.md": "---\ncross_refs: []\n---\n# A\n",
        "b.md": "---\ncross_refs: []\n---\n# B\n",
        "c.md": "---\ncross_refs: []\n---\n# C\n",
    }
    sandbox = _make_sandbox(files)

    lint_library(sandbox, llm_fn=None, page_filter={"a.md", "b.md"})

    read_paths = [call.args[0] for call in sandbox.read.call_args_list]
    assert set(read_paths) == {"a.md", "b.md"}
    assert "c.md" not in read_paths


# ─── AC-9: front-matter parser is forgiving ───────────────────────────


def test_parse_pages_extracts_cross_refs_from_front_matter() -> None:
    files = {
        "a.md": "---\ncross_refs: [b.md, c.md]\n---\n# body of a\n",
    }
    sandbox = _make_sandbox(files)
    pages = parse_pages(sandbox)
    assert len(pages) == 1
    assert pages[0].path == "a.md"
    assert pages[0].cross_refs == ("b.md", "c.md")
    assert pages[0].body.startswith("# body of a")


def test_parse_pages_handles_missing_front_matter() -> None:
    files = {
        "raw.md": "# No front matter\nJust body.\n",
    }
    sandbox = _make_sandbox(files)
    pages = parse_pages(sandbox)
    assert len(pages) == 1
    assert pages[0].cross_refs == ()
    assert pages[0].front_matter == {}
    assert pages[0].body.startswith("# No front matter")


def test_parse_pages_skips_non_markdown_files() -> None:
    files = {
        "note.md": "---\ncross_refs: []\n---\n",
        "binary.bin": "ignored",
    }
    sandbox = _make_sandbox(files)
    pages = parse_pages(sandbox)
    assert [p.path for p in pages] == ["note.md"]


def test_parse_pages_handles_unterminated_front_matter() -> None:
    files = {
        "broken.md": "---\ncross_refs: [a.md]\n# no closing delimiter\n",
    }
    sandbox = _make_sandbox(files)
    pages = parse_pages(sandbox)
    # Unterminated front-matter → treated as no front-matter, no crash.
    assert len(pages) == 1
    assert pages[0].cross_refs == ()


# ─── Module surface sanity ────────────────────────────────────────────


def test_public_api_surface() -> None:
    assert hasattr(lint_mod, "lint_library")
    assert hasattr(lint_mod, "LintFinding")
    assert hasattr(lint_mod, "LibraryPage")
    assert hasattr(lint_mod, "detect_orphans")
    assert hasattr(lint_mod, "detect_stale_refs")
    assert hasattr(lint_mod, "detect_contradictions")
    assert hasattr(lint_mod, "detect_outdated_claims")
