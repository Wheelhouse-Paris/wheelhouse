"""Unit tests for the FR13 post-ingest consistency gate (Story 13-11).

Every test in this file covers an AC in
``_bmad-output/implementation-artifacts/wh/13-11-post-ingest-consistency-check.md``.

The unit tests use the same ``MagicMock(spec=LibrarySandbox)`` double
pattern as ``test_library_ingest_limits.py`` — a mock sandbox backed by
a small in-memory recorder so we can precisely control both the
"existing pages" set that the consistency gate sees and the draft batch
returned by a fake summarizer.
"""

from __future__ import annotations

import contextlib
from typing import Any
from unittest.mock import MagicMock

import pytest

from wheelhouse.errors import LibrarySkillError
from wheelhouse.skills import library_ingest as ingest_mod
from wheelhouse.skills.library_ingest import (
    LIBRARY_INGEST_INCONSISTENT_CROSS_REFS,
    LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG,
    LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION,
    LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG,
    OPTIONAL_PARAMS,
    PageDraft,
    SummarizerResult,
    _check_ingest_consistency,
    _is_allow_slug_reuse,
    _is_unsafe_slug,
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


def _run_ingest(
    sandbox: MagicMock,
    *,
    source_content: str = "body",
    extra_params: dict[str, str] | None = None,
    invocation_id: str = "inv-test",
) -> Any:
    params: dict[str, str] = {
        "source_type": "text",
        "source_ref": "src.txt",
        "source_content": source_content,
    }
    if extra_params:
        params.update(extra_params)
    return run_library_ingest(
        sandbox,
        params,
        library_status="enabled",
        invocation_id=invocation_id,
    )


# ─── OPTIONAL_PARAMS extension ────────────────────────────────────────


def test_allow_slug_reuse_is_in_optional_params() -> None:
    assert "allow_slug_reuse" in OPTIONAL_PARAMS


# ─── _is_allow_slug_reuse helper ──────────────────────────────────────


@pytest.mark.parametrize(
    "value,expected",
    [
        ("true", True),
        ("True", True),
        ("TRUE", True),
        ("1", True),
        ("yes", True),
        ("on", True),
        ("false", False),
        ("0", False),
        ("no", False),
        ("", False),
        ("maybe", False),
        (None, False),
    ],
)
def test_is_allow_slug_reuse_truthy_table(value: Any, expected: bool) -> None:
    params: dict[str, Any] = {}
    if value is not None:
        params["allow_slug_reuse"] = value
    assert _is_allow_slug_reuse(params) is expected


# ─── _is_unsafe_slug helper ───────────────────────────────────────────


@pytest.mark.parametrize(
    "slug,is_unsafe",
    [
        ("notes/page.md", False),
        ("a.md", False),
        ("deep/nested/page.md", False),
        ("", True),
        (".", True),
        ("..", True),
        ("../escape.md", True),
        ("ok/../bad.md", True),
        ("no-suffix", True),
        ("notes/file.txt", True),
        ("notes//file.md", True),
        ("has\x00nul.md", True),
    ],
)
def test_is_unsafe_slug_table(slug: str, is_unsafe: bool) -> None:
    # Normalize first — the gate sees normalized slugs.
    normalized = ingest_mod._normalize_page_path(slug)
    unsafe, _reason = _is_unsafe_slug(normalized)
    assert unsafe is is_unsafe


def test_is_unsafe_slug_skips_absolute_after_normalization() -> None:
    # _normalize_page_path strips leading slashes, so an "absolute"
    # draft path is folded into a safe relative one by the time the
    # gate sees it. This documents that behaviour explicitly.
    assert ingest_mod._normalize_page_path("/absolute.md") == "absolute.md"
    unsafe, _reason = _is_unsafe_slug("absolute.md")
    assert unsafe is False


# ─── AC-1: All cross-refs resolve within batch — success ─────────────


def test_ac1_cross_refs_resolve_within_batch() -> None:
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="Profile",
            body="p",
            cross_refs=["clients/acme/contacts.md"],
        ),
        PageDraft(
            path="clients/acme/contacts.md",
            title="Contacts",
            body="c",
            cross_refs=[],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac1")

    assert result.success is True
    assert recorder.tx_entered == 1
    assert recorder.tx_exited_ok == 1
    assert recorder.tx_rolled_back == 0
    # Both drafted pages landed.
    assert "clients/acme/profile.md" in recorder.files
    assert "clients/acme/contacts.md" in recorder.files


# ─── AC-2: Cross-ref to existing library page — success ──────────────


def test_ac2_cross_ref_resolves_to_existing_page() -> None:
    drafts = [
        PageDraft(
            path="clients/acme/notes.md",
            title="Notes",
            body="n",
            cross_refs=["clients/acme/profile.md"],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder(
        initial_files={"clients/acme/profile.md": "# Profile\n"}
    )
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac2")

    assert result.success is True
    assert recorder.tx_rolled_back == 0


# ─── AC-3: Dangling cross-ref → LIBRARY_INGEST_INCONSISTENT_CROSS_REFS


def test_ac3_dangling_cross_ref_rejected_and_rolled_back() -> None:
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="P",
            body="p",
            cross_refs=["clients/ghost/profile.md"],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac3")

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_CROSS_REFS
    # Error message lists the offending pair.
    assert "clients/acme/profile.md" in (result.error_message or "")
    assert "clients/ghost/profile.md" in (result.error_message or "")
    # Transaction opened then rolled back — no partial state.
    assert recorder.tx_entered == 1
    assert recorder.tx_rolled_back == 1
    assert recorder.tx_exited_ok == 0
    assert recorder.files == {}


# ─── AC-4: Multiple dangling refs all listed ─────────────────────────


def test_ac4_multiple_dangling_refs_all_listed() -> None:
    drafts = [
        PageDraft(
            path="a.md",
            title="A",
            body="a",
            cross_refs=["ghost1.md", "ghost2.md"],
        ),
        PageDraft(
            path="b.md",
            title="B",
            body="b",
            cross_refs=["ghost3.md"],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac4")

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_CROSS_REFS
    msg = result.error_message or ""
    assert "ghost1.md" in msg
    assert "ghost2.md" in msg
    assert "ghost3.md" in msg
    assert "3" in msg  # count of dangling refs


def test_ac4b_excessive_dangling_refs_are_clipped() -> None:
    # Build 20 dangling refs from a single page.
    ghosts = [f"ghost{i}.md" for i in range(20)]
    drafts = [
        PageDraft(path="hub.md", title="H", body="h", cross_refs=ghosts),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac4b")
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_CROSS_REFS
    msg = result.error_message or ""
    # Clip marker present and mentions "more".
    assert "more" in msg
    # Count in the message is the total (20), not the shown count.
    assert "20" in msg


# ─── AC-5: Duplicate slugs in batch → DUPLICATE_SLUG ─────────────────


def test_ac5_duplicate_slug_in_batch_rejected() -> None:
    drafts = [
        PageDraft(path="dup.md", title="d1", body="b1", cross_refs=[]),
        PageDraft(path="dup.md", title="d2", body="b2", cross_refs=[]),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac5")

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG
    assert "dup.md" in (result.error_message or "")
    assert recorder.tx_rolled_back == 1
    assert recorder.files == {}


def test_ac5b_duplicate_after_normalization() -> None:
    # Leading slash normalization should collapse these to the same slug.
    drafts = [
        PageDraft(path="notes/x.md", title="x", body="b", cross_refs=[]),
        PageDraft(path="/notes/x.md", title="x", body="b", cross_refs=[]),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac5b")
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG


# ─── AC-6: Unsafe slug → UNSAFE_SLUG ─────────────────────────────────


@pytest.mark.parametrize(
    "bad_slug",
    [
        "",
        ".",
        "..",
        "../escape.md",
        "ok/../bad.md",
        "no-suffix",
        "notes/file.txt",
    ],
)
def test_ac6_unsafe_slug_rejected(bad_slug: str) -> None:
    drafts = [PageDraft(path=bad_slug, title="x", body="x", cross_refs=[])]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac6")

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG
    assert recorder.tx_rolled_back == 1
    assert recorder.files == {}


# ─── AC-7: Slug collision with existing page → COLLISION ─────────────


def test_ac7_slug_collision_with_existing_page_rejected() -> None:
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="P",
            body="new body",
            cross_refs=[],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder(
        initial_files={"clients/acme/profile.md": "old body"}
    )
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac7")

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION
    assert "clients/acme/profile.md" in (result.error_message or "")
    assert "allow_slug_reuse" in (result.error_message or "")
    # Rollback — existing page is untouched.
    assert recorder.tx_rolled_back == 1
    assert recorder.files == {"clients/acme/profile.md": "old body"}


# ─── AC-8: allow_slug_reuse=true allows deliberate update ────────────


def test_ac8_allow_slug_reuse_allows_update() -> None:
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="P",
            body="new body",
            cross_refs=[],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder(
        initial_files={"clients/acme/profile.md": "old body"}
    )
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(
        sandbox,
        extra_params={"allow_slug_reuse": "true"},
        invocation_id="inv-ac8",
    )

    assert result.success is True
    # Page was updated in place.
    assert "new body" in recorder.files["clients/acme/profile.md"]
    # Recorded as an update, not a creation.
    md = recorder.last_commit_metadata
    assert "clients/acme/profile.md" in md["pages_updated"]
    assert "clients/acme/profile.md" not in md["pages_created"]


# ─── AC-9: Consistency gate runs AFTER the page-count block gate ─────


def test_ac9_library_full_beats_cross_refs() -> None:
    # 500 seeded pages → LIBRARY_FULL hard cap. Draft has a dangling
    # cross-ref too, but LIBRARY_FULL must fire first.
    initial = {
        f"seeded/page_{i:04d}.md": f"# page {i}\n"
        for i in range(ingest_mod.PAGE_COUNT_BLOCK)
    }
    drafts = [
        PageDraft(
            path="new.md",
            title="N",
            body="b",
            cross_refs=["ghost.md"],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder(initial_files=initial)
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac9")

    assert result.success is False
    # LIBRARY_FULL, not CROSS_REFS — the order of gates is pinned.
    assert result.error_code == ingest_mod.LIBRARY_INGEST_LIBRARY_FULL


# ─── AC-10: run_library_ingest translates all four codes ─────────────


def test_ac10_translation_of_all_inconsistent_codes() -> None:
    # CROSS_REFS
    set_summarizer(_fake_summarizer(
        [PageDraft(path="a.md", title="a", body="b", cross_refs=["ghost.md"])]
    ))
    r = _TxRecorder()
    sb = _make_sandbox_mock(r)
    res = _run_ingest(sb, invocation_id="i-1")
    assert res.error_code == LIBRARY_INGEST_INCONSISTENT_CROSS_REFS
    assert res.invocation_id == "i-1"
    assert res.skill_name == "library_ingest"

    # DUPLICATE_SLUG
    set_summarizer(_fake_summarizer([
        PageDraft(path="d.md", title="d", body="b", cross_refs=[]),
        PageDraft(path="d.md", title="d", body="b", cross_refs=[]),
    ]))
    r = _TxRecorder()
    sb = _make_sandbox_mock(r)
    res = _run_ingest(sb, invocation_id="i-2")
    assert res.error_code == LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG
    assert res.invocation_id == "i-2"

    # UNSAFE_SLUG
    set_summarizer(_fake_summarizer(
        [PageDraft(path="../x.md", title="x", body="b", cross_refs=[])]
    ))
    r = _TxRecorder()
    sb = _make_sandbox_mock(r)
    res = _run_ingest(sb, invocation_id="i-3")
    assert res.error_code == LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG
    assert res.invocation_id == "i-3"

    # SLUG_COLLISION
    set_summarizer(_fake_summarizer(
        [PageDraft(path="p.md", title="p", body="b", cross_refs=[])]
    ))
    r = _TxRecorder(initial_files={"p.md": "old"})
    sb = _make_sandbox_mock(r)
    res = _run_ingest(sb, invocation_id="i-4")
    assert res.error_code == LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION
    assert res.invocation_id == "i-4"


# ─── AC-11: Gate leaves no partial state on failure ──────────────────


def test_ac11_no_partial_state_on_dangling_ref_with_mixed_batch() -> None:
    drafts = [
        PageDraft(path="good1.md", title="g1", body="b", cross_refs=[]),
        PageDraft(path="good2.md", title="g2", body="b", cross_refs=[]),
        PageDraft(
            path="bad.md",
            title="bad",
            body="b",
            cross_refs=["ghost.md"],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    recorder = _TxRecorder(
        initial_files={"pre-existing.md": "# old\n"}
    )
    initial_snapshot = dict(recorder.files)
    sandbox = _make_sandbox_mock(recorder)

    result = _run_ingest(sandbox, invocation_id="inv-ac11")

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_CROSS_REFS
    # Rollback counters match: opened + rolled back, never committed.
    assert recorder.tx_entered == 1
    assert recorder.tx_rolled_back == 1
    assert recorder.tx_exited_ok == 0
    # Sandbox state is identical to the pre-ingest snapshot — the
    # gate fired before any draft write landed and none of the good
    # pages appear in recorder.files.
    assert recorder.files == initial_snapshot


# ─── Direct helper test — _check_ingest_consistency on empty batch ───


def test_check_ingest_consistency_empty_batch_is_noop() -> None:
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    # Should not raise on an empty draft list.
    _check_ingest_consistency(sandbox, [], allow_slug_reuse=False)


def test_check_ingest_consistency_direct_raise_on_dangle() -> None:
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    drafts = [
        PageDraft(path="a.md", title="a", body="b", cross_refs=["ghost.md"]),
    ]
    with pytest.raises(LibrarySkillError) as exc_info:
        _check_ingest_consistency(sandbox, drafts, allow_slug_reuse=False)
    assert exc_info.value.code == LIBRARY_INGEST_INCONSISTENT_CROSS_REFS
