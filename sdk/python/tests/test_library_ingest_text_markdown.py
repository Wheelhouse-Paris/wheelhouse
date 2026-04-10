"""Unit + integration tests for the text / markdown ingest pipeline (Story 13-8).

Every test in this file covers an AC in
``_bmad-output/implementation-artifacts/wh/13-8-text-markdown-ingest-with-llm-summarization.md``.

The unit tests use a ``MagicMock(spec=LibrarySandbox)`` double whose
``transaction`` method is a real :mod:`contextlib` context manager
backed by a recorder — no real git, no filesystem I/O outside
``tmp_path``. AC-12 (no-git contract) is honoured by the mock ``spec``:
any call to a method the real ``LibrarySandbox`` does not expose would
raise ``AttributeError`` at test time.

A single integration test at the bottom uses a real ``LibrarySandbox``
rooted at ``tmp_path`` with ``git_enabled=True`` to verify the
end-to-end transaction + commit + on-disk page layout.
"""

from __future__ import annotations

import contextlib
import os
import subprocess
from typing import Any
from unittest.mock import MagicMock

import pytest

from wheelhouse.errors import LibrarySkillError, PathEscapeError
from wheelhouse.skills import library_ingest as ingest_mod
from wheelhouse.skills.library_ingest import (
    LIBRARY_INGEST_BINARY_REJECTED,
    LIBRARY_INGEST_NO_SUMMARIZER,
    LIBRARY_INGEST_SOURCE_NOT_FOUND,
    LIBRARY_INGEST_SUMMARIZER_FAILED,
    LIBRARY_INGEST_UNSUPPORTED_TYPE,
    PageDraft,
    SummarizerResult,
    run_library_ingest,
    set_summarizer,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox, TransactionHandle


# ─── Shared fixtures ──────────────────────────────────────────────────


@pytest.fixture(autouse=True)
def _reset_summarizer() -> Any:
    """Every test starts with NO summarizer wired. Teardown clears."""
    set_summarizer(None)
    yield
    set_summarizer(None)


class _TxRecorder:
    """Minimal in-memory transaction recorder used by the mock sandbox.

    Mirrors the on-disk semantics of ``LibrarySandbox.transaction`` just
    enough for the pipeline to run: writes go into ``files``, existing
    files before the transaction started stay in ``initial_files``, and
    the committed ``commit_metadata`` is captured on normal exit.
    """

    def __init__(self, initial_files: dict[str, str] | None = None) -> None:
        self.initial_files: dict[str, str] = dict(initial_files or {})
        self.files: dict[str, str] = dict(self.initial_files)
        self.tx_entered = 0
        self.tx_exited_ok = 0
        self.tx_rolled_back = 0
        self.last_operation: str | None = None
        self.last_summary: str | None = None
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
        recorder.last_operation = operation
        recorder.last_summary = summary
        handle = TransactionHandle()
        try:
            yield handle
        except BaseException:
            recorder.tx_rolled_back += 1
            # Roll back in-memory: restore to initial files.
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


def _make_fake_summarizer(
    drafts: list[PageDraft],
    *,
    tokens_used: int = 100,
    summary: str = "summarized Acme brief",
) -> Any:
    calls: list[tuple[str, str, str | None, list[str]]] = []

    def _fake(
        source_text: str,
        source_name: str,
        user_hint: str | None,
        existing_pages: list[str],
    ) -> SummarizerResult:
        calls.append((source_text, source_name, user_hint, existing_pages))
        return SummarizerResult(
            drafts=drafts, tokens_used=tokens_used, summary=summary
        )

    _fake.calls = calls  # type: ignore[attr-defined]
    return _fake


# ─── AC-1: Happy path — single transaction, two pages written ─────────


def test_ac1_happy_path_writes_two_pages_in_single_transaction() -> None:
    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="Acme profile",
            body="Acme is a widget company.",
            cross_refs=["clients/acme/contract.md"],
        ),
        PageDraft(
            path="clients/acme/contract.md",
            title="Acme contract",
            body="Signed 2026-03-01.",
            cross_refs=["clients/acme/profile.md", "index.md"],
        ),
    ]
    set_summarizer(_make_fake_summarizer(drafts, tokens_used=4242))

    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "brief.md",
            "source_content": "# Acme\n\n...",
        },
        library_status="enabled",
        invocation_id="inv-001",
    )

    assert result.success is True
    assert result.invocation_id == "inv-001"
    assert result.skill_name == "library_ingest"
    # Transaction opened exactly once.
    assert recorder.tx_entered == 1
    assert recorder.tx_exited_ok == 1
    assert recorder.last_operation == "ingest"
    # Both pages written via sandbox.write + index.md (three entries).
    written_paths = [p for p, _c in recorder.writes]
    assert "clients/acme/profile.md" in written_paths
    assert "clients/acme/contract.md" in written_paths
    assert "index.md" in written_paths
    # commit_metadata
    md = recorder.last_commit_metadata
    assert md["sources"] == ["brief.md"]
    assert "clients/acme/profile.md" in md["pages_created"]
    assert "clients/acme/contract.md" in md["pages_created"]
    assert md["pages_created"] == sorted(md["pages_created"])
    # cross_refs total: 1 + 2 = 3
    assert md["cross_references_added"] == 3
    # SkillResult piggyback fields (13-27). Count is the number of
    # content pages the summarizer drafted — index.md housekeeping is
    # excluded from the metered count.
    assert result.library_page_count == 2
    assert result.library_tokens == 4242
    assert result.library_last_ingest_at is not None
    # ISO-8601 UTC with trailing Z
    assert result.library_last_ingest_at.endswith("Z")
    assert "T" in result.library_last_ingest_at


# ─── AC-2: YAML front-matter shape ────────────────────────────────────


def test_ac2_page_has_yaml_front_matter_with_required_keys() -> None:
    drafts = [
        PageDraft(
            path="notes/alpha.md",
            title="Alpha",
            body="Body paragraph.\n",
            cross_refs=["index.md", "clients/acme/profile.md"],
        ),
    ]
    set_summarizer(_make_fake_summarizer(drafts))
    # Pre-seed the cross-ref target so the 13-11 consistency gate is
    # happy — this test is about front-matter rendering, not dangle
    # detection.
    recorder = _TxRecorder(
        initial_files={"clients/acme/profile.md": "# Acme\n"}
    )
    sandbox = _make_sandbox_mock(recorder)

    run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "brief.md",
            "source_content": "# Brief\n",
        },
        library_status="enabled",
        invocation_id="inv-002",
    )

    content = recorder.files["notes/alpha.md"]
    # Front-matter block comes first.
    assert content.startswith("---\n")
    # Contains required keys in order.
    fm_end = content.index("---\n", 4) + 4
    front_matter = content[:fm_end]
    assert 'source: "brief.md"' in front_matter
    assert "ingest_date:" in front_matter
    assert "cross_refs:" in front_matter
    # ingest_date format YYYY-MM-DDTHH:MM:SSZ
    import re
    m = re.search(
        r"ingest_date: (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z)", front_matter
    )
    assert m is not None, front_matter
    # cross_refs lists both refs in order (JSON-quoted)
    assert '"index.md"' in front_matter
    assert '"clients/acme/profile.md"' in front_matter
    # Blank line after front-matter then body.
    after_fm = content[fm_end:]
    assert after_fm.startswith("\n")
    assert "Body paragraph." in after_fm


# ─── AC-3: Updated pages go into pages_updated, not pages_created ─────


def test_ac3_updated_page_recorded_as_update() -> None:
    drafts = [
        PageDraft(
            path="existing.md",
            title="Existing",
            body="Refreshed body.",
            cross_refs=[],
        ),
    ]
    set_summarizer(_make_fake_summarizer(drafts))
    # Pre-populate an existing page.
    recorder = _TxRecorder(
        initial_files={
            "existing.md": "---\nsource: \"old\"\n---\n\nOld body\n"
        }
    )
    sandbox = _make_sandbox_mock(recorder)

    run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "brief.txt",
            "source_content": "# brief\n",
            # Story 13-11: the consistency gate treats slug collisions
            # as an error by default. This test is specifically about
            # the update path, so opt in explicitly.
            "allow_slug_reuse": "true",
        },
        library_status="enabled",
        invocation_id="inv-003",
    )

    md = recorder.last_commit_metadata
    assert "existing.md" in md["pages_updated"]
    assert "existing.md" not in md["pages_created"]


# ─── AC-4: index.md is created (and updated) with Ingested sources ────


def test_ac4_index_created_when_missing() -> None:
    drafts = [
        PageDraft(path="a.md", title="Alpha", body="A.", cross_refs=[]),
        PageDraft(path="b.md", title="Beta", body="B.", cross_refs=[]),
    ]
    set_summarizer(_make_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "brief.md",
            "source_content": "# brief\n",
        },
        library_status="enabled",
        invocation_id="inv-004",
    )

    assert "index.md" in recorder.files
    assert "index.md" in recorder.last_commit_metadata["pages_created"]
    index_body = recorder.files["index.md"]
    assert "## Ingested sources" in index_body
    assert "- [Alpha](a.md)" in index_body
    assert "- [Beta](b.md)" in index_body


def test_ac4_index_extended_preserves_prior_entries() -> None:
    drafts = [
        PageDraft(path="c.md", title="Gamma", body="C.", cross_refs=[]),
    ]
    set_summarizer(_make_fake_summarizer(drafts))
    prior_index = (
        "# Library index\n\n"
        "## Ingested sources\n\n"
        "- [Earlier](earlier.md)\n"
    )
    recorder = _TxRecorder(initial_files={"index.md": prior_index})
    sandbox = _make_sandbox_mock(recorder)

    run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "brief.md",
            "source_content": "# brief\n",
        },
        library_status="enabled",
        invocation_id="inv-004b",
    )

    new_index = recorder.files["index.md"]
    assert "- [Earlier](earlier.md)" in new_index
    assert "- [Gamma](c.md)" in new_index
    md = recorder.last_commit_metadata
    assert "index.md" in md["pages_updated"]
    assert "index.md" not in md["pages_created"]


# ─── AC-5: Binary content rejected cleanly ────────────────────────────


def test_ac5_binary_source_rejected_before_transaction() -> None:
    set_summarizer(_make_fake_summarizer([]))  # won't be called
    recorder = _TxRecorder(
        initial_files={"blob.bin": "good\x00bad"}
    )
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {"source_type": "text", "source_ref": "blob.bin"},
        library_status="enabled",
        invocation_id="inv-005",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_BINARY_REJECTED
    assert recorder.tx_entered == 0
    # NFR9 — no filesystem path in the error message.
    assert "blob.bin" not in result.error_message
    assert "binary" in result.error_message.lower()


# ─── AC-6: Inline source_content bypasses file read ───────────────────


def test_ac6_source_content_bypasses_file_resolution() -> None:
    drafts = [PageDraft(path="p.md", title="P", body="X.", cross_refs=[])]
    fake = _make_fake_summarizer(drafts)
    set_summarizer(fake)
    recorder = _TxRecorder()  # "from-chat.md" does NOT exist
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "from-chat.md",
            "source_content": "# Hello\n\nworld\n",
        },
        library_status="enabled",
        invocation_id="inv-006",
    )

    assert result.success is True
    # sandbox.read should NEVER have been called — inline bypass.
    sandbox.read.assert_not_called()
    # Summarizer received the inline text and the basename.
    src_text, src_name, _hint, _existing = fake.calls[0]  # type: ignore[attr-defined]
    assert src_text == "# Hello\n\nworld\n"
    assert src_name == "from-chat.md"


# ─── AC-7: Path escape → SOURCE_NOT_FOUND, no path echoed ─────────────


def test_ac7_path_escape_returns_source_not_found() -> None:
    set_summarizer(_make_fake_summarizer([]))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    # Simulate a real sandbox's path-escape behaviour.
    def _escape_read(rel_path: str) -> str:
        raise PathEscapeError(debug_hash="deadbeef")

    sandbox.read.side_effect = _escape_read

    result = run_library_ingest(
        sandbox,
        {"source_type": "text", "source_ref": "../etc/passwd"},
        library_status="enabled",
        invocation_id="inv-007",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_SOURCE_NOT_FOUND
    assert "../etc/passwd" not in result.error_message
    assert "passwd" not in result.error_message
    assert recorder.tx_entered == 0


# ─── AC-8: Unsupported source_type (url only after 13-9) ──────────────
#
# Story 13-8 originally parametrized this with ``["pdf", "url"]``. Story
# 13-9 added the real PDF branch, so "pdf" is now supported and only
# "url" remains unimplemented. The parametrize list is narrowed rather
# than deleted to preserve the AC-8 coverage shape.


@pytest.mark.parametrize("bad_type", ["url"])
def test_ac8_unsupported_source_type_refused(bad_type: str) -> None:
    set_summarizer(_make_fake_summarizer([]))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {"source_type": bad_type, "source_ref": "doc.xxx"},
        library_status="enabled",
        invocation_id="inv-008",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_UNSUPPORTED_TYPE
    # Safe to echo the closed-enum value.
    assert bad_type in result.error_message
    assert recorder.tx_entered == 0


# ─── AC-9: No summarizer → clean refusal ──────────────────────────────


def test_ac9_no_summarizer_clean_refusal() -> None:
    # autouse fixture already set summarizer to None.
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "brief.md",
            "source_content": "# brief\n",
        },
        library_status="enabled",
        invocation_id="inv-009",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_NO_SUMMARIZER
    assert recorder.tx_entered == 0


# ─── AC-10: Summarizer failure rolls back ────────────────────────────


def test_ac10_summarizer_failure_rolls_back() -> None:
    def _boom(*_a: Any, **_kw: Any) -> SummarizerResult:
        raise RuntimeError("model overloaded — /secret/path")

    set_summarizer(_boom)
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "text",
            "source_ref": "brief.txt",
            "source_content": "content",
        },
        library_status="enabled",
        invocation_id="inv-010",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_SUMMARIZER_FAILED
    # Original exception message never echoed (may leak upstream payloads).
    assert "/secret/path" not in result.error_message
    assert "model overloaded" not in result.error_message
    # No partial writes.
    assert recorder.files == {}
    # Transaction not even entered (summarizer is called pre-tx).
    assert recorder.tx_entered == 0


# ─── AC-11 / AC-12: translation into SkillResult ─────────────────────


def test_ac11_success_path_echoes_invocation_and_skill_name() -> None:
    drafts = [PageDraft(path="q.md", title="Q", body="Q.", cross_refs=[])]
    set_summarizer(_make_fake_summarizer(drafts, tokens_used=1234))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "brief.md",
            "source_content": "# brief\n",
        },
        library_status="enabled",
        invocation_id="i-1",
    )
    assert result.success is True
    assert result.invocation_id == "i-1"
    assert result.skill_name == "library_ingest"
    assert result.library_tokens == 1234


def test_ac12_binary_error_translation_echoes_ids() -> None:
    set_summarizer(_make_fake_summarizer([]))
    recorder = _TxRecorder(initial_files={"blob.bin": "ok\x00bad"})
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {"source_type": "text", "source_ref": "blob.bin"},
        library_status="enabled",
        invocation_id="i-2",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_BINARY_REJECTED
    assert result.invocation_id == "i-2"
    assert result.skill_name == "library_ingest"


# ─── Helper-unit tests ────────────────────────────────────────────────


def test_utc_now_iso_has_z_suffix_and_no_microseconds() -> None:
    ts = ingest_mod._utc_now_iso()
    assert ts.endswith("Z")
    import re
    assert re.match(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$", ts)


def test_normalize_page_path_strips_leading_slash_and_backslashes() -> None:
    assert ingest_mod._normalize_page_path("/a/b.md") == "a/b.md"
    assert ingest_mod._normalize_page_path("a\\b.md") == "a/b.md"
    assert ingest_mod._normalize_page_path("x.md") == "x.md"


def test_render_page_preserves_body_newlines() -> None:
    out = ingest_mod._render_page(
        body="line1\nline2",
        source_name="s.md",
        ingest_ts="2026-04-10T12:00:00Z",
        cross_refs=["a.md"],
    )
    assert out.startswith("---\n")
    assert out.endswith("line1\nline2\n")


# ─── Integration test — real LibrarySandbox + git ─────────────────────


def test_integration_real_sandbox_commits_ingest(tmp_path: Any) -> None:
    """Uses a real LibrarySandbox with git_enabled=True under tmp_path.

    Verifies that the ingest pipeline opens a transaction, writes
    pages to disk, and the resulting single commit lands with the
    expected files tracked in git.
    """
    # Skip if git is not on PATH — the SDK runs in minimal CI too.
    if subprocess.call(
        ["git", "--version"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ) != 0:
        pytest.skip("git not available")

    root = tmp_path / ".library"
    root.mkdir()
    sandbox = LibrarySandbox(
        str(root),
        git_enabled=True,
        agent_name="test-agent",
    )

    drafts = [
        PageDraft(
            path="clients/acme/profile.md",
            title="Acme profile",
            body="Acme is a widget company.\n",
            # Story 13-11: cross-refs must resolve. ``index.md`` is
            # always a valid link target after ingest, so we use it
            # here rather than a hallucinated contract page.
            cross_refs=["index.md"],
        ),
    ]
    set_summarizer(_make_fake_summarizer(drafts, tokens_used=99))

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "markdown",
            "source_ref": "brief.md",
            "source_content": "# Acme\n\nwidget co\n",
        },
        library_status="enabled",
        invocation_id="integ-1",
    )

    assert result.success is True, result.error_message
    # Page file exists on disk with front-matter.
    page_path = root / "clients" / "acme" / "profile.md"
    assert page_path.exists()
    content = page_path.read_text(encoding="utf-8")
    assert content.startswith("---\n")
    assert 'source: "brief.md"' in content
    assert "Acme is a widget company." in content
    # index.md exists.
    assert (root / "index.md").exists()
    # Single commit captured via git log.
    log = subprocess.check_output(
        ["git", "-C", str(root), "log", "--oneline"],
        text=True,
    )
    assert len(log.strip().splitlines()) == 1
    # Working tree clean after the transaction commit.
    status = subprocess.check_output(
        ["git", "-C", str(root), "status", "--porcelain"],
        text=True,
    )
    assert status == ""
