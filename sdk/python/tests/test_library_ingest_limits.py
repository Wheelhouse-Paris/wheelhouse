"""Unit tests for the FR11 size gate and FR12 page-count gates (Story 13-10).

Every test in this file covers an AC in
``_bmad-output/implementation-artifacts/wh/13-10-size-and-page-count-limit-enforcement.md``.

The unit tests use the same ``MagicMock(spec=LibrarySandbox)`` double
pattern as ``test_library_ingest_text_markdown.py`` — a mock sandbox
backed by a small in-memory recorder so we can precisely control the
"existing pages" count that the FR12 gates see.
"""

from __future__ import annotations

import contextlib
import logging
from typing import Any
from unittest.mock import MagicMock

import pytest

from wheelhouse.errors import LibrarySkillError
from wheelhouse.skills import library_ingest as ingest_mod
from wheelhouse.skills.library_ingest import (
    LIBRARY_INGEST_LIBRARY_FULL,
    LIBRARY_INGEST_SOURCE_TOO_LARGE,
    MAX_INGEST_WORDS,
    PAGE_COUNT_BLOCK,
    PAGE_COUNT_WARN,
    PageDraft,
    SummarizerResult,
    _count_library_pages,
    run_library_ingest,
    set_summarizer,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox, TransactionHandle


# ─── Shared fixtures (duplicated from test_library_ingest_text_markdown) ──


@pytest.fixture(autouse=True)
def _reset_summarizer() -> Any:
    set_summarizer(None)
    yield
    set_summarizer(None)


class _TxRecorder:
    def __init__(self, initial_files: dict[str, str] | None = None) -> None:
        self.initial_files: dict[str, str] = dict(initial_files or {})
        self.files: dict[str, str] = dict(self.initial_files)
        self.tx_entered = 0
        self.tx_exited_ok = 0
        self.tx_rolled_back = 0
        self.last_commit_metadata: dict[str, Any] = {}
        self.writes: list[tuple[str, str]] = []


def _make_sandbox_mock(recorder: _TxRecorder) -> MagicMock:
    sandbox = MagicMock(spec=LibrarySandbox)

    def _read(rel_path: str) -> str:
        if rel_path in recorder.files:
            return recorder.files[rel_path]
        raise FileNotFoundError(rel_path)

    def _write(rel_path: str, content: str) -> None:
        recorder.files[rel_path] = content
        recorder.writes.append((rel_path, content))

    def _exists(rel_path: str) -> bool:
        return rel_path in recorder.files

    def _list(rel_path: str = ".") -> list[str]:
        return sorted(recorder.files.keys())

    @contextlib.contextmanager
    def _transaction(operation: str, summary: str):  # type: ignore[no-untyped-def]
        recorder.tx_entered += 1
        handle = TransactionHandle()
        try:
            yield handle
        except BaseException:
            recorder.tx_rolled_back += 1
            recorder.files = dict(recorder.initial_files)
            recorder.writes = []
            raise
        recorder.tx_exited_ok += 1
        recorder.last_commit_metadata = dict(handle.commit_metadata)

    sandbox.read.side_effect = _read
    sandbox.write.side_effect = _write
    sandbox.exists.side_effect = _exists
    sandbox.list.side_effect = _list
    sandbox.transaction.side_effect = _transaction
    return sandbox


def _fake_summarizer(drafts: list[PageDraft], *, tokens_used: int = 100) -> Any:
    calls: list[tuple[Any, ...]] = []

    def _fake(source_text, source_name, user_hint, existing_pages):  # type: ignore[no-untyped-def]
        calls.append((source_text, source_name, user_hint, existing_pages))
        return SummarizerResult(
            drafts=drafts, tokens_used=tokens_used, summary="s"
        )

    _fake.calls = calls  # type: ignore[attr-defined]
    return _fake


def _seed_existing_pages(count: int) -> dict[str, str]:
    """Return a dict of ``count`` fake existing page files."""
    return {
        f"seeded/page_{i:04d}.md": f"# page {i}\n" for i in range(count)
    }


# ─── AC-1: Source under 50K words proceeds unchanged ──────────────────


def test_ac1_small_source_proceeds() -> None:
    drafts = [PageDraft(path="notes/hi.md", title="Hi", body="hi.")]
    summarizer = _fake_summarizer(drafts)
    set_summarizer(summarizer)

    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "small.txt",
            "source_content": "word " * 1000,
        },
        library_status="enabled",
        invocation_id="inv-1",
    )

    assert result.success is True
    assert len(summarizer.calls) == 1  # type: ignore[attr-defined]
    assert recorder.tx_entered == 1


# ─── AC-2: Source over 50K words is rejected ──────────────────────────


def test_ac2_oversize_source_rejected() -> None:
    summarizer = _fake_summarizer(
        [PageDraft(path="a.md", title="a", body="a")]
    )
    set_summarizer(summarizer)

    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    big = "word " * 50_001  # 50_001 whitespace-separated words
    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "big.txt",
            "source_content": big,
        },
        library_status="enabled",
        invocation_id="inv-2",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_SOURCE_TOO_LARGE
    assert "50001" in (result.error_message or "")
    assert str(MAX_INGEST_WORDS) in (result.error_message or "")
    # Token estimate is present — 50001 * 1.3 + 500 = 65501 (int trunc).
    assert "tokens" in (result.error_message or "")
    # Summarizer never called, transaction never opened.
    assert len(summarizer.calls) == 0  # type: ignore[attr-defined]
    assert recorder.tx_entered == 0
    # NFR9: no path leakage.
    assert "big.txt" not in (result.error_message or "")


# ─── AC-3: accept_large=true overrides with warning ───────────────────


def test_ac3_accept_large_override(caplog: pytest.LogCaptureFixture) -> None:
    drafts = [PageDraft(path="big/notes.md", title="Big", body="body")]
    summarizer = _fake_summarizer(drafts)
    set_summarizer(summarizer)

    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    big = "word " * 60_000
    with caplog.at_level(logging.WARNING, logger="wheelhouse.library_ingest"):
        result = run_library_ingest(
            sandbox,
            {
                "source_type": "text",
                "source_ref": "big.txt",
                "source_content": big,
                "accept_large": "true",
            },
            library_status="enabled",
            invocation_id="inv-3",
        )

    assert result.success is True
    assert len(summarizer.calls) == 1  # type: ignore[attr-defined]
    # Warning was emitted with word count + token estimate.
    warn_msgs = [r.message for r in caplog.records if r.levelno == logging.WARNING]
    assert any("accept_large" in m for m in warn_msgs)
    assert any("60000" in m for m in warn_msgs)


@pytest.mark.parametrize("value", ["true", "True", "TRUE", "1", "yes", "Yes", "on"])
def test_ac3b_accept_large_truthy_variants(value: str) -> None:
    drafts = [PageDraft(path="x.md", title="x", body="x")]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "b.txt",
            "source_content": "word " * 55_000,
            "accept_large": value,
        },
        library_status="enabled",
        invocation_id="inv-3b",
    )
    assert result.success is True


@pytest.mark.parametrize("value", ["false", "no", "0", "", "maybe"])
def test_ac3c_accept_large_falsy_variants(value: str) -> None:
    set_summarizer(_fake_summarizer([PageDraft(path="x.md", title="x", body="x")]))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "b.txt",
            "source_content": "word " * 55_000,
            "accept_large": value,
        },
        library_status="enabled",
        invocation_id="inv-3c",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_SOURCE_TOO_LARGE


# ─── AC-4: Library below warn threshold produces no advisory ──────────


def test_ac4_below_warn_threshold_no_warning(
    caplog: pytest.LogCaptureFixture,
) -> None:
    drafts = [PageDraft(path="new.md", title="new", body="body")]
    set_summarizer(_fake_summarizer(drafts))

    recorder = _TxRecorder(initial_files=_seed_existing_pages(10))
    sandbox = _make_sandbox_mock(recorder)

    with caplog.at_level(logging.WARNING, logger="wheelhouse.library_ingest"):
        result = run_library_ingest(
            sandbox,
            {
                "source_type": "text",
                "source_ref": "s.txt",
                "source_content": "hi",
            },
            library_status="enabled",
            invocation_id="inv-4",
        )

    assert result.success is True
    assert not any(
        "approaching 500-page soft cap" in r.message for r in caplog.records
    )


# ─── AC-5: Library at 200+ pages logs advisory but still ingests ──────


def test_ac5_warn_threshold_advisory(caplog: pytest.LogCaptureFixture) -> None:
    drafts = [PageDraft(path="new_at_warn.md", title="new", body="body")]
    set_summarizer(_fake_summarizer(drafts))

    recorder = _TxRecorder(initial_files=_seed_existing_pages(PAGE_COUNT_WARN))
    sandbox = _make_sandbox_mock(recorder)

    with caplog.at_level(logging.WARNING, logger="wheelhouse.library_ingest"):
        result = run_library_ingest(
            sandbox,
            {
                "source_type": "text",
                "source_ref": "s.txt",
                "source_content": "hi",
            },
            library_status="enabled",
            invocation_id="inv-5",
        )

    assert result.success is True
    assert any(
        "approaching 500-page soft cap" in r.message for r in caplog.records
    )
    # The new page was actually written.
    assert any(p == "new_at_warn.md" for p, _ in recorder.writes)


# ─── AC-6: Library at 500 pages + 1 new page → LIBRARY_FULL ───────────


def test_ac6_hard_block_at_cap() -> None:
    drafts = [PageDraft(path="overflow.md", title="overflow", body="body")]
    set_summarizer(_fake_summarizer(drafts))

    recorder = _TxRecorder(
        initial_files=_seed_existing_pages(PAGE_COUNT_BLOCK)
    )
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "s.txt",
            "source_content": "hi",
        },
        library_status="enabled",
        invocation_id="inv-6",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_LIBRARY_FULL
    assert "500" in (result.error_message or "")
    # Transaction was entered (block happens inside) then rolled back.
    assert recorder.tx_entered == 1
    assert recorder.tx_rolled_back == 1
    assert recorder.tx_exited_ok == 0
    # No writes persisted (rolled back).
    assert "overflow.md" not in recorder.files
    # NFR9: no source path echoed.
    assert "s.txt" not in (result.error_message or "")


# ─── AC-7: Library at 499 + 1 new = 500 succeeds (at cap, not over) ───


def test_ac7_exactly_at_cap_succeeds() -> None:
    drafts = [PageDraft(path="final.md", title="final", body="body")]
    set_summarizer(_fake_summarizer(drafts))

    recorder = _TxRecorder(
        initial_files=_seed_existing_pages(PAGE_COUNT_BLOCK - 1)
    )
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "s.txt",
            "source_content": "hi",
        },
        library_status="enabled",
        invocation_id="inv-7",
    )

    assert result.success is True
    # Final count == 500 (499 seeded + 1 new + index.md which is excluded).
    assert _count_library_pages(sandbox) == PAGE_COUNT_BLOCK


# ─── AC-8: Library at 500, drafted page is an UPDATE, succeeds ────────


def test_ac8_update_at_cap_succeeds() -> None:
    # The summarizer drafts a page whose path is ALREADY in the Library —
    # an update, not a new page — so the count does not grow.
    existing = _seed_existing_pages(PAGE_COUNT_BLOCK)
    target_path = "seeded/page_0042.md"
    assert target_path in existing

    drafts = [PageDraft(path=target_path, title="updated", body="new body")]
    set_summarizer(_fake_summarizer(drafts))

    recorder = _TxRecorder(initial_files=existing)
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "s.txt",
            "source_content": "hi",
        },
        library_status="enabled",
        invocation_id="inv-8",
    )

    assert result.success is True
    # Final count still 500.
    assert _count_library_pages(sandbox) == PAGE_COUNT_BLOCK
    # The target page was actually rewritten (body contains the new body).
    assert "new body" in recorder.files[target_path]


# ─── AC-9: index.md and .git/ files are not counted ───────────────────


def test_ac9_page_count_excludes_index_and_git() -> None:
    recorder = _TxRecorder(
        initial_files={
            "index.md": "# Library index\n",
            "clients/acme/profile.md": "# Acme\n",
            ".git/HEAD": "ref: refs/heads/main\n",
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    assert _count_library_pages(sandbox) == 1


# ─── AC-10: run_library_ingest translates the new codes into SkillResult ──


def test_ac10_translation_of_too_large_and_full() -> None:
    # SOURCE_TOO_LARGE — covered by AC-2 already, but assert both
    # fields on a freshly-set-up call for clarity.
    set_summarizer(_fake_summarizer([PageDraft(path="x.md", title="x", body="x")]))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    r = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "s.txt",
            "source_content": "word " * (MAX_INGEST_WORDS + 1),
        },
        library_status="enabled",
        invocation_id="inv-10a",
    )
    assert r.success is False
    assert r.error_code == LIBRARY_INGEST_SOURCE_TOO_LARGE
    assert r.invocation_id == "inv-10a"
    assert r.skill_name == "library_ingest"

    # LIBRARY_FULL — seed 500 pages and try adding one.
    recorder2 = _TxRecorder(
        initial_files=_seed_existing_pages(PAGE_COUNT_BLOCK)
    )
    sandbox2 = _make_sandbox_mock(recorder2)
    r2 = run_library_ingest(
        sandbox2,
        {
            "source_type": "text",
            "source_ref": "s.txt",
            "source_content": "hi",
        },
        library_status="enabled",
        invocation_id="inv-10b",
    )
    assert r2.success is False
    assert r2.error_code == LIBRARY_INGEST_LIBRARY_FULL
    assert r2.invocation_id == "inv-10b"


# ─── AC-11: Size gate operates on POST-extraction text ────────────────


def test_ac11_size_gate_runs_after_pdf_extraction(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The size gate MUST run on the extracted text, not the PDF bytes.

    We stub out ``_resolve_pdf_text`` so the PDF branch returns a
    60_000-word synthetic text blob, then assert the size gate raises
    LIBRARY_INGEST_SOURCE_TOO_LARGE on THAT word count without the
    summarizer ever being called.
    """
    big_extracted_text = "word " * 60_000
    calls: list[str] = []

    def _fake_resolve_pdf_text(sandbox, parameters):  # type: ignore[no-untyped-def]
        calls.append("pdf_resolve_called")
        return big_extracted_text, "brief.pdf", False

    monkeypatch.setattr(
        ingest_mod, "_resolve_pdf_text", _fake_resolve_pdf_text
    )

    # Summarizer that tracks whether it was called.
    summarizer = _fake_summarizer(
        [PageDraft(path="x.md", title="x", body="x")]
    )
    set_summarizer(summarizer)

    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "brief.pdf",
            "source_content": "%PDF-stub",  # stubbed resolver ignores this
        },
        library_status="enabled",
        invocation_id="inv-11",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_SOURCE_TOO_LARGE
    # PDF resolver WAS called (extraction ran) but summarizer was NOT.
    assert calls == ["pdf_resolve_called"]
    assert len(summarizer.calls) == 0  # type: ignore[attr-defined]
    assert recorder.tx_entered == 0
