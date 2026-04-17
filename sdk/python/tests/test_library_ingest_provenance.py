"""Unit tests for FR40 source provenance tracking (Story 13-13).

Every test in this file covers an AC in
``_bmad-output/implementation-artifacts/wh/13-13-source-provenance-tracking.md``.

Fixture pattern mirrors ``test_library_ingest_source_dedup.py`` (13-12):
a ``MagicMock(spec=LibrarySandbox)`` backed by an in-memory
``_TxRecorder`` so the tests can precisely control both the starting
file set (including a seeded ``.provenance.json``) and the transaction
rollback semantics.
"""

from __future__ import annotations

import contextlib
import json
import logging
from typing import Any
from unittest.mock import MagicMock

import pytest

from wheelhouse.errors import LibrarySkillError
from wheelhouse.skills import library_ingest as ingest_mod
from wheelhouse.skills.library_ingest import (
    LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE,
    LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG,
    PageDraft,
    SummarizerResult,
    _PROVENANCE_PATH,
    _check_ingest_consistency,
    _load_provenance,
    _serialize_provenance,
    _update_provenance,
    get_provenance,
    run_library_ingest,
    set_summarizer,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox, TransactionHandle


# ─── Shared fixtures (mirrored from test_library_ingest_source_dedup) ──


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
        # Event log for AC-5: ordered sequence of recorder events
        # ("tx_enter", ("write", path), "tx_exit_ok", "tx_rollback").
        self.events: list[Any] = []


def _make_sandbox_mock(recorder: _TxRecorder) -> MagicMock:
    sandbox = MagicMock(spec=LibrarySandbox)

    def _read(rel_path: str) -> str:
        if rel_path in recorder.files:
            return recorder.files[rel_path]
        raise FileNotFoundError(rel_path)

    def _write(rel_path: str, content: str) -> None:
        recorder.files[rel_path] = content
        recorder.writes.append((rel_path, content))
        recorder.events.append(("write", rel_path))

    def _exists(rel_path: str) -> bool:
        return rel_path in recorder.files

    def _list(rel_path: str = ".") -> list[str]:
        return sorted(recorder.files.keys())

    @contextlib.contextmanager
    def _transaction(operation: str, summary: str):  # type: ignore[no-untyped-def]
        recorder.tx_entered += 1
        recorder.tx_operations.append(operation)
        recorder.events.append("tx_enter")
        handle = TransactionHandle()
        try:
            yield handle
        except BaseException:
            recorder.tx_rolled_back += 1
            recorder.files = dict(recorder.initial_files)
            recorder.writes = []
            recorder.events.append("tx_rollback")
            raise
        recorder.tx_exited_ok += 1
        recorder.last_commit_metadata = dict(handle.commit_metadata)
        recorder.events.append("tx_exit_ok")

    sandbox.read.side_effect = _read
    sandbox.write.side_effect = _write
    sandbox.exists.side_effect = _exists
    sandbox.list.side_effect = _list
    sandbox.transaction.side_effect = _transaction
    return sandbox


def _fake_summarizer(
    drafts: list[PageDraft], *, tokens_used: int = 100
) -> Any:
    def _fake(source_text, source_name, user_hint, existing_pages):  # type: ignore[no-untyped-def]
        return SummarizerResult(
            drafts=drafts, tokens_used=tokens_used, summary="summarized"
        )

    return _fake


def _run_ingest(
    sandbox: MagicMock,
    *,
    source_type: str = "markdown",
    source_ref: str = "notes.md",
    source_content: str = "body content",
    invocation_id: str = "inv-prov",
) -> Any:
    return run_library_ingest(
        sandbox,
        {
            "source_type": source_type,
            "source_ref": source_ref,
            "source_content": source_content,
        },
        library_status="enabled",
        invocation_id=invocation_id,
    )


def _seed_page_with_fm(
    body: str,
    *,
    source: str,
    source_type: str = "markdown",
    ingest_ts: str = "2026-04-01T00:00:00Z",
    cross_refs: list[str] | None = None,
) -> str:
    refs = cross_refs or []
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


# ─── AC-1: New page carries the four-field front-matter ───────────────


def test_ac1_new_page_has_four_field_front_matter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        ingest_mod, "_utc_now_iso", lambda: "2026-04-10T00:00:00Z"
    )
    drafts = [
        PageDraft(
            path="topic.md",
            title="Topic",
            body="Body content.",
            cross_refs=[],
        )
    ]
    set_summarizer(_fake_summarizer(drafts))
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)

    result = _run_ingest(sandbox, source_type="markdown", source_ref="notes.md")
    assert result.success, result.error_message

    content = rec.files["topic.md"]
    assert content.startswith("---\n")
    # Extract the front-matter block.
    fm_end = content.index("---\n", 4)
    fm = content[:fm_end]
    # Five top-level keys in pinned order (source_file added by 15-1-1).
    lines = [ln for ln in fm.splitlines() if ln and not ln.startswith("---")]
    keys = [ln.split(":", 1)[0] for ln in lines]
    assert keys == ["source", "source_type", "ingest_date", "cross_refs", "source_file"], keys
    assert 'source: "notes.md"' in fm
    assert 'source_type: "markdown"' in fm
    assert "ingest_date: 2026-04-10T00:00:00Z" in fm
    assert "cross_refs: [" in fm


# ─── AC-2: Missing-provenance consistency gate raises ─────────────────


def test_ac2_missing_provenance_gate_raises(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Fault-inject: force _render_page to produce a payload with no
    # ``source`` field so the 13-13 check fires.
    def _broken_render(**kwargs: Any) -> str:
        # Drop the source line entirely.
        return (
            "---\n"
            "source_type: \"text\"\n"
            f"ingest_date: {kwargs['ingest_ts']}\n"
            "cross_refs: []\n"
            "---\n"
            "\nbody\n"
        )

    monkeypatch.setattr(ingest_mod, "_render_page", _broken_render)
    monkeypatch.setattr(
        ingest_mod, "_utc_now_iso", lambda: "2026-04-10T00:00:00Z"
    )

    drafts = [
        PageDraft(
            path="topic.md", title="T", body="body", cross_refs=[]
        )
    ]
    set_summarizer(_fake_summarizer(drafts))
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)

    result = _run_ingest(sandbox)
    assert not result.success
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE
    assert rec.tx_rolled_back == 1
    # No sidecar written on the rolled-back transaction.
    assert _PROVENANCE_PATH not in rec.files


# ─── AC-3: First ingest creates a .provenance.json entry ──────────────


def test_ac3_first_ingest_creates_sidecar_entry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        ingest_mod, "_utc_now_iso", lambda: "2026-04-10T12:00:00Z"
    )
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="Profile",
            body="p",
            cross_refs=[],
        ),
        PageDraft(
            path="clients/acme/contacts.md",
            title="Contacts",
            body="c",
            cross_refs=[],
        ),
    ]
    set_summarizer(_fake_summarizer(drafts))
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)

    _run_ingest(sandbox, source_type="text", source_ref="acme.md")

    assert _PROVENANCE_PATH in rec.files
    sidecar = json.loads(rec.files[_PROVENANCE_PATH])
    assert sidecar["version"] == 1
    assert set(sidecar["entries"].keys()) == {
        "clients/acme/profile.md",
        "clients/acme/contacts.md",
    }
    e = sidecar["entries"]["clients/acme/profile.md"]
    assert e == {
        "source": "acme.md",
        "source_type": "text",
        "first_ingest_date": "2026-04-10T12:00:00Z",
        "last_ingest_date": "2026-04-10T12:00:00Z",
        "history": ["2026-04-10T12:00:00Z"],
    }


# ─── AC-4: Re-ingest preserves first_ingest_date, appends history ─────


def test_ac4_reingest_preserves_first_date_appends_history(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seeded_page = _seed_page_with_fm(
        "Old body.",
        source="acme.md",
        source_type="markdown",
        ingest_ts="2026-04-01T00:00:00Z",
    )
    seeded_sidecar = _serialize_provenance(
        {
            "version": 1,
            "entries": {
                "clients/acme/profile.md": {
                    "source": "acme.md",
                    "source_type": "markdown",
                    "first_ingest_date": "2026-04-01T00:00:00Z",
                    "last_ingest_date": "2026-04-01T00:00:00Z",
                    "history": ["2026-04-01T00:00:00Z"],
                }
            },
        }
    )
    rec = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": seeded_page,
            _PROVENANCE_PATH: seeded_sidecar,
        }
    )
    sandbox = _make_sandbox_mock(rec)

    monkeypatch.setattr(
        ingest_mod, "_utc_now_iso", lambda: "2026-04-10T00:00:00Z"
    )
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="Profile",
            body="New body.",
            cross_refs=[],
        )
    ]
    set_summarizer(_fake_summarizer(drafts))

    _run_ingest(sandbox, source_type="markdown", source_ref="acme.md")

    sidecar = json.loads(rec.files[_PROVENANCE_PATH])
    e = sidecar["entries"]["clients/acme/profile.md"]
    assert e["first_ingest_date"] == "2026-04-01T00:00:00Z"
    assert e["last_ingest_date"] == "2026-04-10T00:00:00Z"
    assert e["history"] == [
        "2026-04-01T00:00:00Z",
        "2026-04-10T00:00:00Z",
    ]


# ─── AC-5: Sidecar write is inside the same transaction ───────────────


def test_ac5_sidecar_write_inside_transaction() -> None:
    drafts = [
        PageDraft(path="t.md", title="T", body="b", cross_refs=[])
    ]
    set_summarizer(_fake_summarizer(drafts))
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)

    _run_ingest(sandbox)

    assert rec.tx_entered == 1
    assert rec.tx_exited_ok == 1
    # The events list captures the ordering: tx_enter, several writes,
    # tx_exit_ok. Both t.md and the sidecar must be writes sandwiched
    # between the tx_enter and tx_exit_ok events.
    tx_enter_idx = rec.events.index("tx_enter")
    tx_exit_idx = rec.events.index("tx_exit_ok")
    between = rec.events[tx_enter_idx + 1 : tx_exit_idx]
    written_paths = [e[1] for e in between if isinstance(e, tuple) and e[0] == "write"]
    assert "t.md" in written_paths
    assert _PROVENANCE_PATH in written_paths


# ─── AC-6: Rollback leaves .provenance.json untouched ─────────────────


def test_ac6_rollback_leaves_sidecar_byte_identical() -> None:
    initial_sidecar = _serialize_provenance(
        {
            "version": 1,
            "entries": {
                "existing.md": {
                    "source": "old.md",
                    "source_type": "text",
                    "first_ingest_date": "2026-04-01T00:00:00Z",
                    "last_ingest_date": "2026-04-01T00:00:00Z",
                    "history": ["2026-04-01T00:00:00Z"],
                }
            },
        }
    )
    rec = _TxRecorder(
        initial_files={_PROVENANCE_PATH: initial_sidecar}
    )
    sandbox = _make_sandbox_mock(rec)

    # Dangling cross_ref triggers the 13-11 gate → rollback.
    drafts = [
        PageDraft(
            path="new.md",
            title="New",
            body="b",
            cross_refs=["nowhere.md"],
        )
    ]
    set_summarizer(_fake_summarizer(drafts))

    result = _run_ingest(sandbox)
    assert not result.success
    assert rec.tx_rolled_back == 1
    # _TxRecorder rolls back by restoring initial_files.
    assert rec.files[_PROVENANCE_PATH] == initial_sidecar


# ─── AC-7: _load_provenance tolerates corrupt sidecar ─────────────────


def test_ac7_load_provenance_tolerates_corrupt(
    caplog: pytest.LogCaptureFixture,
) -> None:
    rec = _TxRecorder(
        initial_files={_PROVENANCE_PATH: "{{{ not json"}
    )
    sandbox = _make_sandbox_mock(rec)
    with caplog.at_level(logging.WARNING, logger="wheelhouse.library_ingest"):
        result = _load_provenance(sandbox)
    assert result == {}
    assert any("not valid JSON" in rec.message for rec in caplog.records)


# ─── AC-8: _load_provenance tolerates missing sidecar ─────────────────


def test_ac8_load_provenance_tolerates_missing(
    caplog: pytest.LogCaptureFixture,
) -> None:
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)
    with caplog.at_level(logging.WARNING, logger="wheelhouse.library_ingest"):
        result = _load_provenance(sandbox)
    assert result == {}
    # No warnings on the happy missing-file path.
    assert not caplog.records


# ─── AC-9: _update_provenance is pure (no mutation of input) ──────────


def test_ac9_update_provenance_does_not_mutate_input() -> None:
    existing = {
        "version": 1,
        "entries": {
            "a.md": {
                "source": "s1",
                "source_type": "text",
                "first_ingest_date": "2026-01-01T00:00:00Z",
                "last_ingest_date": "2026-01-01T00:00:00Z",
                "history": ["2026-01-01T00:00:00Z"],
            }
        },
    }
    updated = _update_provenance(
        existing, [("a.md", "s1", "text")], "2026-04-10T00:00:00Z"
    )
    assert updated is not existing
    # Input untouched.
    assert existing["entries"]["a.md"]["last_ingest_date"] == "2026-01-01T00:00:00Z"
    assert existing["entries"]["a.md"]["history"] == ["2026-01-01T00:00:00Z"]
    # Output reflects the update.
    assert updated["entries"]["a.md"]["last_ingest_date"] == "2026-04-10T00:00:00Z"
    assert updated["entries"]["a.md"]["history"] == [
        "2026-01-01T00:00:00Z",
        "2026-04-10T00:00:00Z",
    ]


# ─── AC-10: get_provenance returns entry for existing slug ────────────


def test_ac10_get_provenance_returns_entry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        ingest_mod, "_utc_now_iso", lambda: "2026-04-10T00:00:00Z"
    )
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="P",
            body="b",
            cross_refs=[],
        )
    ]
    set_summarizer(_fake_summarizer(drafts))
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)
    _run_ingest(sandbox, source_type="text", source_ref="acme.md")

    entry = get_provenance(sandbox, "clients/acme/profile.md")
    assert entry is not None
    assert entry["source"] == "acme.md"
    assert entry["source_type"] == "text"
    assert entry["first_ingest_date"] == "2026-04-10T00:00:00Z"
    assert entry["last_ingest_date"] == "2026-04-10T00:00:00Z"
    assert entry["history"] == ["2026-04-10T00:00:00Z"]


# ─── AC-11: get_provenance returns None for unknown slug ──────────────


def test_ac11_get_provenance_unknown_slug_returns_none() -> None:
    drafts = [PageDraft(path="t.md", title="T", body="b", cross_refs=[])]
    set_summarizer(_fake_summarizer(drafts))
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)
    _run_ingest(sandbox)

    assert get_provenance(sandbox, "does/not/exist.md") is None


# ─── AC-12: Serialized sidecar is byte-stable ─────────────────────────


def test_ac12_serialize_is_byte_stable() -> None:
    sidecar_a = _update_provenance(
        {},
        [("b.md", "b-src", "text"), ("a.md", "a-src", "markdown")],
        "2026-04-10T00:00:00Z",
    )
    sidecar_b = _update_provenance(
        {},
        [("a.md", "a-src", "markdown"), ("b.md", "b-src", "text")],
        "2026-04-10T00:00:00Z",
    )
    s_a = _serialize_provenance(sidecar_a)
    s_b = _serialize_provenance(sidecar_b)
    assert s_a == s_b
    assert s_a.endswith("\n")
    # Round-trip parse.
    parsed = json.loads(s_a)
    assert parsed == sidecar_a
    # Top-level keys in sorted order: "entries" < "version".
    top_keys = list(parsed.keys())
    assert top_keys == sorted(top_keys)


# ─── AC-13: source_type survives front-matter round-trip ──────────────


def test_ac13_source_type_round_trips_through_front_matter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        ingest_mod, "_utc_now_iso", lambda: "2026-04-10T00:00:00Z"
    )
    # Use the PDF path so source_type=="pdf". Easiest: call _summarize_and_write
    # directly via a monkey-patched resolver, OR go through _render_page.
    rendered = ingest_mod._render_page(
        body="b",
        source_name="doc.pdf",
        source_type="pdf",
        ingest_ts="2026-04-10T00:00:00Z",
        cross_refs=[],
    )
    fm, _body = ingest_mod._parse_front_matter_local(rendered)
    assert fm["source_type"] == "pdf"
    assert fm["source"] == "doc.pdf"
    assert fm["ingest_date"] == "2026-04-10T00:00:00Z"


# ─── AC-14: Re-ingest with 13-12 dedup shares txn with sidecar write ──


def test_ac14_reingest_dedup_shares_transaction_with_sidecar(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seeded_page = _seed_page_with_fm(
        "Old body.",
        source="acme.md",
        source_type="markdown",
        ingest_ts="2026-04-01T00:00:00Z",
    )
    seeded_sidecar = _serialize_provenance(
        {
            "version": 1,
            "entries": {
                "clients/acme/profile.md": {
                    "source": "acme.md",
                    "source_type": "markdown",
                    "first_ingest_date": "2026-04-01T00:00:00Z",
                    "last_ingest_date": "2026-04-01T00:00:00Z",
                    "history": ["2026-04-01T00:00:00Z"],
                }
            },
        }
    )
    rec = _TxRecorder(
        initial_files={
            "clients/acme/profile.md": seeded_page,
            _PROVENANCE_PATH: seeded_sidecar,
        }
    )
    sandbox = _make_sandbox_mock(rec)
    monkeypatch.setattr(
        ingest_mod, "_utc_now_iso", lambda: "2026-04-10T00:00:00Z"
    )
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="Profile",
            body="Fresh body.",
            cross_refs=[],
        )
    ]
    set_summarizer(_fake_summarizer(drafts))

    _run_ingest(sandbox, source_type="markdown", source_ref="acme.md")

    assert rec.tx_operations == ["re-ingest"]
    assert rec.tx_entered == 1
    # Both the page rewrite AND the sidecar rewrite happened in the
    # single transaction.
    written_paths = [p for (p, _c) in rec.writes]
    assert "clients/acme/profile.md" in written_paths
    assert _PROVENANCE_PATH in written_paths

    sidecar = json.loads(rec.files[_PROVENANCE_PATH])
    history = sidecar["entries"]["clients/acme/profile.md"]["history"]
    assert history == [
        "2026-04-01T00:00:00Z",
        "2026-04-10T00:00:00Z",
    ]


# ─── AC-15: Consistency gate runs BEFORE the sidecar write ────────────


def test_ac15_gate_failure_skips_sidecar_write() -> None:
    drafts = [
        PageDraft(
            path="../escape.md",  # unsafe slug → 13-11 gate
            title="T",
            body="b",
            cross_refs=[],
        )
    ]
    set_summarizer(_fake_summarizer(drafts))
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)

    result = _run_ingest(sandbox)
    assert not result.success
    assert result.error_code == LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG
    assert rec.tx_rolled_back == 1
    written_paths = [p for (p, _c) in rec.writes]
    assert _PROVENANCE_PATH not in written_paths


# ─── Bonus: _check_ingest_consistency direct call ─────────────────────


def test_check_consistency_rejects_missing_source_field() -> None:
    """Direct unit test for the 13-13 gate check without going through the pipeline."""
    drafts = [
        PageDraft(path="page.md", title="P", body="b", cross_refs=[])
    ]
    rendered = {
        "page.md": (
            "---\n"
            "source_type: \"text\"\n"
            "ingest_date: 2026-04-10T00:00:00Z\n"
            "cross_refs: []\n"
            "---\n"
            "\nbody\n"
        )
    }
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)
    with pytest.raises(LibrarySkillError) as excinfo:
        _check_ingest_consistency(
            sandbox,
            drafts,
            allow_slug_reuse=False,
            rendered_pages=rendered,
        )
    assert excinfo.value.code == LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE
    assert "source" in str(excinfo.value)


def test_check_consistency_rejects_missing_ingest_date() -> None:
    drafts = [
        PageDraft(path="page.md", title="P", body="b", cross_refs=[])
    ]
    rendered = {
        "page.md": (
            "---\n"
            "source: \"a.md\"\n"
            "source_type: \"text\"\n"
            "cross_refs: []\n"
            "---\n"
            "\nbody\n"
        )
    }
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)
    with pytest.raises(LibrarySkillError) as excinfo:
        _check_ingest_consistency(
            sandbox,
            drafts,
            allow_slug_reuse=False,
            rendered_pages=rendered,
        )
    assert excinfo.value.code == LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE
    assert "ingest_date" in str(excinfo.value)


def test_check_consistency_rendered_pages_none_is_noop() -> None:
    """Legacy callers pass no rendered_pages — the new check must be a no-op."""
    drafts = [
        PageDraft(path="page.md", title="P", body="b", cross_refs=[])
    ]
    rec = _TxRecorder()
    sandbox = _make_sandbox_mock(rec)
    # Should not raise — matches the 13-11 test-calling pattern that
    # exercises the gate without synthesizing rendered content.
    _check_ingest_consistency(
        sandbox,
        drafts,
        allow_slug_reuse=False,
        rendered_pages=None,
    )
