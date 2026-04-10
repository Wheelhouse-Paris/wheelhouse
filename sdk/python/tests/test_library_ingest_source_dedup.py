"""Unit tests for FR16 source dedup on re-ingest (Story 13-12).

Every test in this file covers an AC in
``_bmad-output/implementation-artifacts/wh/13-12-source-dedup-on-reingest.md``.

The unit tests use the same ``MagicMock(spec=LibrarySandbox)`` double
pattern as ``test_library_ingest_consistency.py`` (13-11) — a mock
sandbox backed by a small in-memory recorder so we can precisely
control both the "existing pages" set (including their front-matter)
and the draft batch returned by a fake summarizer.
"""

from __future__ import annotations

import contextlib
from typing import Any
from unittest.mock import MagicMock

import pytest

from wheelhouse.skills import library_ingest as ingest_mod
from wheelhouse.skills.library_ingest import (
    PageDraft,
    SummarizerResult,
    _ExistingSourcePage,
    _find_existing_by_source,
    _parse_front_matter_local,
    _summarizer_accepts_dedup_arg,
    run_library_ingest,
    set_summarizer,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox, TransactionHandle


# ─── Shared fixtures (duplicated from test_library_ingest_consistency) ──


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
        self.tx_operations: list[str] = []
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
        recorder.tx_operations.append(operation)
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


def _seed_page(
    body: str,
    *,
    source: str,
    source_type: str = "text",
    cross_refs: list[str] | None = None,
    ingest_ts: str = "2026-04-10T12:00:00Z",
) -> str:
    """Render a seeded page with the same front-matter shape as 13-13.

    Story 13-13 adds ``source_type`` as a required front-matter field.
    The default ``"text"`` preserves pre-13-13 test behaviour for
    fixtures that do not care about the source-type dimension.
    """
    refs = cross_refs or []
    import json

    refs_yaml = ", ".join(json.dumps(r) for r in refs)
    return (
        "---\n"
        f"source: {json.dumps(source)}\n"
        f"source_type: {json.dumps(source_type)}\n"
        f"ingest_date: {ingest_ts}\n"
        f"cross_refs: [{refs_yaml}]\n"
        "---\n"
        "\n"
        + (body if body.endswith("\n") else body + "\n")
    )


def _fake_summarizer_recording(
    drafts: list[PageDraft], *, tokens_used: int = 100
) -> Any:
    """5-arg recording summarizer used by AC-2 / AC-6 to observe dedup args."""
    calls: list[tuple[Any, ...]] = []

    def _fake(
        source_text,
        source_name,
        user_hint,
        existing_pages,
        existing_source_pages=None,
    ):  # type: ignore[no-untyped-def]
        calls.append(
            (
                source_text,
                source_name,
                user_hint,
                existing_pages,
                existing_source_pages,
            )
        )
        return SummarizerResult(
            drafts=drafts, tokens_used=tokens_used, summary="s"
        )

    _fake.calls = calls  # type: ignore[attr-defined]
    return _fake


def _fake_summarizer_4arg(
    drafts: list[PageDraft], *, tokens_used: int = 100
) -> Any:
    """Pre-13-12 4-arg summarizer — used by AC-7 backward-compat test."""
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
    source_ref: str = "acme.md",
    source_content: str = "body",
    extra_params: dict[str, str] | None = None,
    invocation_id: str = "inv-test",
) -> Any:
    params: dict[str, str] = {
        "source_type": "text",
        "source_ref": source_ref,
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


# ─── _parse_front_matter_local ────────────────────────────────────────


def test_parse_front_matter_local_well_formed() -> None:
    text = (
        '---\nsource: "acme.md"\ncross_refs: ["a.md", "b.md"]\n---\n\n'
        "body line 1\nbody line 2\n"
    )
    fm, body = _parse_front_matter_local(text)
    assert fm["source"] == "acme.md"
    assert fm["cross_refs"] == ["a.md", "b.md"]
    assert body == "body line 1\nbody line 2\n"


def test_parse_front_matter_local_no_front_matter() -> None:
    text = "# just a title\n\nbody\n"
    fm, body = _parse_front_matter_local(text)
    assert fm == {}
    assert body == text


def test_parse_front_matter_local_unterminated() -> None:
    text = '---\nsource: "acme.md"\n\nbody\n'
    fm, body = _parse_front_matter_local(text)
    # Unterminated — parser returns empty fm and full text.
    assert fm == {}
    assert body == text


# ─── _summarizer_accepts_dedup_arg ────────────────────────────────────


def test_summarizer_accepts_dedup_arg_4arg() -> None:
    def four_arg(a, b, c, d):  # type: ignore[no-untyped-def]
        return None

    assert _summarizer_accepts_dedup_arg(four_arg) is False


def test_summarizer_accepts_dedup_arg_5arg() -> None:
    def five_arg(a, b, c, d, e):  # type: ignore[no-untyped-def]
        return None

    assert _summarizer_accepts_dedup_arg(five_arg) is True


def test_summarizer_accepts_dedup_arg_5arg_optional() -> None:
    def five_arg(a, b, c, d, e=None):  # type: ignore[no-untyped-def]
        return None

    assert _summarizer_accepts_dedup_arg(five_arg) is True


def test_summarizer_accepts_dedup_arg_varargs() -> None:
    def varargs(*args, **kwargs):  # type: ignore[no-untyped-def]
        return None

    assert _summarizer_accepts_dedup_arg(varargs) is True


# ─── _find_existing_by_source ─────────────────────────────────────────


def test_find_existing_by_source_empty_library() -> None:
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    assert _find_existing_by_source(sandbox, "acme.md") == []


def test_find_existing_by_source_returns_matches() -> None:
    recorder = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": _seed_page(
                "acme profile body",
                source="acme.md",
                cross_refs=["clients/acme/contacts.md"],
            ),
            "clients/zeta/profile.md": _seed_page(
                "zeta profile body", source="zeta.md"
            ),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    matches = _find_existing_by_source(sandbox, "acme.md")
    assert len(matches) == 1
    assert matches[0].slug == "clients/acme/profile.md"
    assert "acme profile body" in matches[0].body
    assert matches[0].cross_refs == ("clients/acme/contacts.md",)


def test_find_existing_by_source_ac10_robust_to_malformed() -> None:
    """AC-10: malformed / no-front-matter / missing-source pages are skipped."""
    recorder = _TxRecorder(
        initial_files={
            # Well-formed match
            "p1.md": _seed_page("one", source="acme.md"),
            # No front-matter
            "p2.md": "# plain markdown\n\nno front-matter here\n",
            # Front-matter but no source key
            "p3.md": (
                '---\nother_key: "value"\ncross_refs: []\n---\n\nbody\n'
            ),
            # Different source
            "p4.md": _seed_page("four", source="other.md"),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    matches = _find_existing_by_source(sandbox, "acme.md")
    assert [m.slug for m in matches] == ["p1.md"]


def test_find_existing_by_source_skips_index_and_git() -> None:
    recorder = _TxRecorder(
        initial_files={
            "index.md": _seed_page("idx", source="acme.md"),
            ".git/HEAD.md": _seed_page("git", source="acme.md"),
            "real.md": _seed_page("real", source="acme.md"),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    matches = _find_existing_by_source(sandbox, "acme.md")
    assert [m.slug for m in matches] == ["real.md"]


# ─── AC-1: empty library → dedup is a no-op ───────────────────────────


def test_ac1_no_prior_ingest_dedup_is_noop() -> None:
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [PageDraft(path="clients/acme/profile.md", title="Acme", body="new")]
    )
    set_summarizer(fake)

    result = _run_ingest(sandbox, source_ref="acme.md")

    assert result.success, result.error_message
    assert recorder.last_commit_metadata["pages_created"] == [
        "clients/acme/profile.md",
        "index.md",
    ]
    assert recorder.last_commit_metadata["pages_updated"] == []
    assert "source_dedup" not in recorder.last_commit_metadata
    # 5-arg summarizer was called — existing_source_pages should be empty.
    assert fake.calls[0][4] == []
    # Transaction op tag is plain "ingest", not "re-ingest".
    assert recorder.tx_operations == ["ingest"]


# ─── AC-2: prior ingest passes existing pages into summarizer ─────────


def test_ac2_prior_ingest_existing_pages_passed() -> None:
    recorder = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": _seed_page(
                "seeded body\n", source="acme.md", cross_refs=["other.md"]
            ),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [
            PageDraft(
                path="clients/acme/profile.md",
                title="Acme",
                body="merged body",
                cross_refs=["other.md"],
            ),
            # Seed an "other.md" so the cross_ref doesn't dangle.
        ]
    )
    # Seed other.md so cross_refs don't dangle
    recorder.initial_files["other.md"] = _seed_page("other", source="other.md")
    recorder.files["other.md"] = recorder.initial_files["other.md"]
    set_summarizer(fake)

    result = _run_ingest(sandbox, source_ref="acme.md")

    assert result.success, result.error_message
    assert len(fake.calls) == 1
    existing_source_pages = fake.calls[0][4]
    assert isinstance(existing_source_pages, list)
    assert len(existing_source_pages) == 1
    match = existing_source_pages[0]
    assert isinstance(match, _ExistingSourcePage)
    assert match.slug == "clients/acme/profile.md"
    assert "seeded body" in match.body
    assert match.cross_refs == ("other.md",)


# ─── AC-3: same-slug draft over existing source → update, not error ──


def test_ac3_same_slug_reingest_becomes_update() -> None:
    recorder = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": _seed_page(
                "old body", source="acme.md"
            ),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [PageDraft(path="clients/acme/profile.md", title="Acme", body="new body")]
    )
    set_summarizer(fake)

    # NOTE: no allow_slug_reuse in params
    result = _run_ingest(sandbox, source_ref="acme.md")

    assert result.success, result.error_message
    assert recorder.last_commit_metadata["pages_updated"] == [
        "clients/acme/profile.md"
    ]
    assert "clients/acme/profile.md" not in recorder.last_commit_metadata[
        "pages_created"
    ]
    # The page body was overwritten.
    assert "new body" in recorder.files["clients/acme/profile.md"]
    assert "old body" not in recorder.files["clients/acme/profile.md"]
    assert recorder.last_commit_metadata["source_dedup"] is True


# ─── AC-4: dedup is keyed on canonical basename ───────────────────────


def test_ac4_canonical_basename_match() -> None:
    recorder = _TxRecorder(
        initial_files={
            "notes/a.md": _seed_page("old a", source="a.md"),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [PageDraft(path="notes/a.md", title="A", body="merged a")]
    )
    set_summarizer(fake)

    # source_ref includes a subdir — basename-ing should match "a.md"
    result = _run_ingest(sandbox, source_ref="subdir/a.md")

    assert result.success, result.error_message
    assert "notes/a.md" in recorder.last_commit_metadata["pages_updated"]
    assert recorder.last_commit_metadata["source_dedup"] is True


# ─── AC-5: different source, same slug → still a collision error ─────


def test_ac5_different_source_same_slug_still_collides() -> None:
    recorder = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": _seed_page("old", source="acme.md"),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [PageDraft(path="clients/acme/profile.md", title="Acme", body="zeta!")]
    )
    set_summarizer(fake)

    # Different source file — no dedup match, so 13-11 slug-collision fires.
    result = _run_ingest(sandbox, source_ref="zeta.md")

    assert not result.success
    assert result.error_code == "LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION"
    # Transaction rolled back — the existing page is untouched.
    assert "old" in recorder.files["clients/acme/profile.md"]
    assert recorder.tx_rolled_back == 1


# ─── AC-6: multiple existing pages for same source all passed ────────


def test_ac6_multiple_pages_for_source_all_passed() -> None:
    recorder = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": _seed_page("p1", source="acme.md"),
            "clients/acme/contacts.md": _seed_page("p2", source="acme.md"),
            "clients/acme/history.md": _seed_page("p3", source="acme.md"),
            "other.md": _seed_page("noise", source="other.md"),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [
            PageDraft(
                path="clients/acme/profile.md", title="P", body="new p1"
            ),
        ]
    )
    set_summarizer(fake)

    result = _run_ingest(sandbox, source_ref="acme.md")

    assert result.success, result.error_message
    existing_source_pages = fake.calls[0][4]
    slugs = sorted(m.slug for m in existing_source_pages)
    assert slugs == [
        "clients/acme/contacts.md",
        "clients/acme/history.md",
        "clients/acme/profile.md",
    ]


# ─── AC-7: 4-arg summarizer still works ──────────────────────────────


def test_ac7_4arg_summarizer_backward_compat() -> None:
    recorder = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": _seed_page("old", source="acme.md"),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_4arg(
        [PageDraft(path="clients/acme/profile.md", title="P", body="new")]
    )
    set_summarizer(fake)

    result = _run_ingest(sandbox, source_ref="acme.md")

    assert result.success, result.error_message
    # 4-arg summarizer called with exactly 4 args.
    assert len(fake.calls) == 1
    assert len(fake.calls[0]) == 4
    # Dedup still flipped allow_slug_reuse → update, not collision.
    assert recorder.last_commit_metadata["pages_updated"] == [
        "clients/acme/profile.md"
    ]
    assert recorder.last_commit_metadata["source_dedup"] is True


# ─── AC-8: explicit allow_slug_reuse="true" still honoured ───────────


def test_ac8_explicit_allow_slug_reuse_no_match() -> None:
    recorder = _TxRecorder()  # empty library
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [PageDraft(path="clients/acme/profile.md", title="P", body="new")]
    )
    set_summarizer(fake)

    result = _run_ingest(
        sandbox,
        source_ref="acme.md",
        extra_params={"allow_slug_reuse": "true"},
    )

    assert result.success, result.error_message
    # Fresh ingest — no dedup, plain "ingest" op.
    assert recorder.tx_operations == ["ingest"]
    assert "source_dedup" not in recorder.last_commit_metadata


# ─── AC-9: commit subject tagged "re-ingest" ─────────────────────────


def test_ac9_transaction_tagged_reingest() -> None:
    recorder = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": _seed_page("old", source="acme.md"),
        }
    )
    sandbox = _make_sandbox_mock(recorder)
    fake = _fake_summarizer_recording(
        [
            PageDraft(
                path="clients/acme/profile.md", title="P", body="merged"
            )
        ]
    )
    set_summarizer(fake)

    result = _run_ingest(sandbox, source_ref="acme.md")

    assert result.success, result.error_message
    assert recorder.tx_operations == ["re-ingest"]
    assert recorder.last_commit_metadata["source_dedup"] is True
    assert "clients/acme/profile.md" in recorder.last_commit_metadata[
        "pages_updated"
    ]
