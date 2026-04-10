"""Unit tests for the PDF ingest branch (Story 13-9).

Every test in this file covers an AC in
``_bmad-output/implementation-artifacts/wh/13-9-pdf-extraction-for-ingest.md``.

PDF fixtures are built IN-PROCESS with :mod:`pypdf`'s own ``PdfWriter``
so that this repository does not ship a binary blob and so that the
tests exercise a real pypdf parse round-trip. The empty-text PDF is
built by writing an empty page (no ``draw_text``) which produces a
valid structural PDF whose ``page.extract_text()`` returns an empty
string — this is the scanned-image equivalent for AC-2.
"""

from __future__ import annotations

import base64
import contextlib
import io
from typing import Any
from unittest.mock import MagicMock

import pytest

from wheelhouse.errors import LibrarySkillError, PathEscapeError
from wheelhouse.skills import library_ingest as ingest_mod
from wheelhouse.skills.library_ingest import (
    LIBRARY_INGEST_PDF_EMPTY_EXTRACTION,
    LIBRARY_INGEST_PDF_INVALID,
    LIBRARY_INGEST_SOURCE_NOT_FOUND,
    PageDraft,
    SummarizerResult,
    run_library_ingest,
    set_summarizer,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox, TransactionHandle


# ─── Fixtures ─────────────────────────────────────────────────────────


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


def _make_sandbox_mock(
    recorder: _TxRecorder,
    *,
    pdf_files: dict[str, bytes] | None = None,
) -> MagicMock:
    sandbox = MagicMock(spec=LibrarySandbox)
    pdf_files = pdf_files or {}

    def _read(rel_path: str) -> str:
        if rel_path in recorder.files:
            return recorder.files[rel_path]
        raise FileNotFoundError(rel_path)

    def _read_bytes(rel_path: str) -> bytes:
        if rel_path in pdf_files:
            return pdf_files[rel_path]
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
    sandbox.read_bytes.side_effect = _read_bytes
    sandbox.write.side_effect = _write
    sandbox.exists.side_effect = _exists
    sandbox.list.side_effect = _list
    sandbox.transaction.side_effect = _transaction
    return sandbox


def _make_fake_summarizer(
    drafts: list[PageDraft],
    *,
    tokens_used: int = 100,
    summary: str = "summarized brief",
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


# ─── PDF builders ────────────────────────────────────────────────────


def _build_text_pdf(pages: list[str]) -> bytes:
    """Return a real PDF byte-blob with one page per entry in ``pages``.

    Pages are drawn via a tiny hand-rolled content stream that pypdf
    can round-trip through ``page.extract_text()``. The hack with an
    empty list produces a structurally valid PDF whose sole page
    extracts to an empty string — the NFR26 scanned-image equivalent.
    """
    from pypdf import PdfWriter
    from pypdf.generic import (
        ContentStream,
        DecodedStreamObject,
        DictionaryObject,
        NameObject,
        TextStringObject,
    )

    writer = PdfWriter()
    for text in pages:
        writer.add_blank_page(width=612, height=792)
        page = writer.pages[-1]
        # Build a minimal content stream: BT / Tf / Td / Tj / ET.
        if text:
            # Escape parentheses and backslashes per the PDF string spec.
            safe = (
                text.replace("\\", "\\\\")
                .replace("(", "\\(")
                .replace(")", "\\)")
            )
            stream_data = (
                b"BT\n/F1 12 Tf\n72 720 Td\n("
                + safe.encode("latin-1", errors="replace")
                + b") Tj\nET\n"
            )
            content_obj = DecodedStreamObject()
            content_obj.set_data(stream_data)
            # Install a Helvetica font resource so pypdf can decode Tj.
            font_dict = DictionaryObject(
                {
                    NameObject("/Type"): NameObject("/Font"),
                    NameObject("/Subtype"): NameObject("/Type1"),
                    NameObject("/BaseFont"): NameObject("/Helvetica"),
                }
            )
            font_ref = writer._add_object(font_dict)
            resources = DictionaryObject(
                {
                    NameObject("/Font"): DictionaryObject(
                        {NameObject("/F1"): font_ref}
                    )
                }
            )
            page[NameObject("/Resources")] = resources
            page[NameObject("/Contents")] = writer._add_object(content_obj)

    buf = io.BytesIO()
    writer.write(buf)
    return buf.getvalue()


def _build_empty_text_pdf() -> bytes:
    """A valid PDF whose single page extracts to an empty string."""
    return _build_text_pdf([""])


# ─── AC-1: valid text-layer PDF runs through the shared pipeline ─────


def test_ac1_valid_pdf_routes_through_shared_summarizer_and_writer() -> None:
    pdf_bytes = _build_text_pdf(
        ["Hello from page one.", "Hello from page two."]
    )
    drafts = [
        PageDraft(
            path="notes/hello.md",
            title="Hello",
            body="Summary body.",
            cross_refs=[],
        ),
    ]
    fake = _make_fake_summarizer(drafts, tokens_used=321)
    set_summarizer(fake)

    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "brief.pdf",
            "source_content": base64.b64encode(pdf_bytes).decode("ascii"),
        },
        library_status="enabled",
        invocation_id="inv-pdf-1",
    )

    assert result.success is True
    # Summarizer was called with extracted text containing BOTH pages.
    assert len(fake.calls) == 1
    source_text, source_name, user_hint, existing_pages = fake.calls[0]
    assert "Hello from page one" in source_text
    assert "Hello from page two" in source_text
    assert source_name == "brief.pdf"
    # Single transaction, draft written.
    assert recorder.tx_entered == 1
    assert recorder.tx_exited_ok == 1
    written_paths = [p for p, _c in recorder.writes]
    assert "notes/hello.md" in written_paths
    # Piggyback fields populated.
    assert result.library_page_count == 1
    assert result.library_tokens == 321
    assert result.library_last_ingest_at is not None
    assert result.library_last_ingest_at.endswith("Z")
    # No partial-extraction flag on a clean PDF.
    assert "pdf_partial_extraction" not in recorder.last_commit_metadata


# ─── AC-2: empty-text PDF → LIBRARY_INGEST_PDF_EMPTY_EXTRACTION ──────


def test_ac2_empty_text_pdf_raises_empty_extraction_with_pinned_message() -> None:
    pdf_bytes = _build_empty_text_pdf()
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="x.md", title="x", body="x", cross_refs=[])]
        )
    )
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "scan.pdf",
            "source_content": base64.b64encode(pdf_bytes).decode("ascii"),
        },
        library_status="enabled",
        invocation_id="inv-pdf-empty",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_PDF_EMPTY_EXTRACTION
    # NFR26 verbatim message.
    assert (
        result.error_message
        == "No extractable text found — OCR not supported in v1."
    )
    # No transaction opened.
    assert recorder.tx_entered == 0
    # Error message does not echo source_ref.
    assert "scan.pdf" not in result.error_message


# ─── AC-3: binary-but-not-PDF (PNG renamed) → PDF_INVALID ────────────


def test_ac3_png_bytes_renamed_as_pdf_raises_pdf_invalid() -> None:
    png_bytes = b"\x89PNG\r\n\x1a\n" + b"not really a png either"
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="x.md", title="x", body="x", cross_refs=[])]
        )
    )

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "/home/alice/fake.pdf",
            "source_content": base64.b64encode(png_bytes).decode("ascii"),
        },
        library_status="enabled",
        invocation_id="inv-pdf-png",
    )

    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_PDF_INVALID
    # NFR9 — path never echoed.
    assert "/home/alice/fake.pdf" not in result.error_message
    assert "fake.pdf" not in result.error_message
    assert recorder.tx_entered == 0


# ─── AC-4: partial extraction — some pages raise, others decode ─────


def test_ac4_partial_extraction_flag_set_on_commit_metadata() -> None:
    pdf_bytes = _build_text_pdf(["Good page.", "Second good page."])

    class _PagesProxy:
        def __init__(self, real_pages: Any) -> None:
            self._real = list(real_pages)

        def __iter__(self) -> Any:
            for i, page in enumerate(self._real):
                if i == 0:
                    yield page
                else:
                    # Second iteration: raise to simulate a bad page.
                    class _BoomPage:
                        def extract_text(self) -> str:
                            raise RuntimeError("boom")

                    yield _BoomPage()

    original_reader = ingest_mod.__dict__.get("pypdf", None)

    from unittest.mock import patch

    import pypdf

    real_reader = pypdf.PdfReader(io.BytesIO(pdf_bytes))

    class _FakeReader:
        is_encrypted = False
        pages = _PagesProxy(real_reader.pages)

    def _fake_pdfreader(_stream: Any) -> _FakeReader:
        return _FakeReader()

    drafts = [
        PageDraft(path="n.md", title="n", body="b", cross_refs=[]),
    ]
    set_summarizer(_make_fake_summarizer(drafts))
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    with patch("pypdf.PdfReader", _fake_pdfreader):
        result = run_library_ingest(
            sandbox,
            {
                "source_type": "pdf",
                "source_ref": "partial.pdf",
                "source_content": base64.b64encode(pdf_bytes).decode("ascii"),
            },
            library_status="enabled",
            invocation_id="inv-pdf-partial",
        )

    assert result.success is True, result.error_message
    assert recorder.last_commit_metadata.get("pdf_partial_extraction") is True
    # The summarizer got only the first page's text.
    # (fake.calls recorded via the fixture return closure.)
    assert any(
        "Good page." in call[0]
        for call in _get_fake_calls(ingest_mod.get_summarizer())
    )

    _ = original_reader  # keep the import hook happy


def _get_fake_calls(fn: Any) -> list[tuple[str, str, str | None, list[str]]]:
    return getattr(fn, "calls", [])


# ─── AC-5: sandbox.read_bytes is used when source_content is absent ──


def test_ac5_pdf_branch_calls_read_bytes_not_read() -> None:
    pdf_bytes = _build_text_pdf(["Content from disk."])
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(
        recorder, pdf_files={"brief.pdf": pdf_bytes}
    )
    drafts = [PageDraft(path="n.md", title="n", body="b", cross_refs=[])]
    set_summarizer(_make_fake_summarizer(drafts))

    result = run_library_ingest(
        sandbox,
        {"source_type": "pdf", "source_ref": "brief.pdf"},
        library_status="enabled",
        invocation_id="inv-pdf-disk",
    )

    assert result.success is True, result.error_message
    sandbox.read_bytes.assert_called_once_with("brief.pdf")
    sandbox.read.assert_not_called()


# ─── AC-6: path escape on a PDF source_ref → SOURCE_NOT_FOUND ────────


def test_ac6_pdf_path_escape_becomes_source_not_found() -> None:
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    sandbox.read_bytes.side_effect = PathEscapeError(debug_hash="abc")
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="x.md", title="x", body="x", cross_refs=[])]
        )
    )

    result = run_library_ingest(
        sandbox,
        {"source_type": "pdf", "source_ref": "../etc/passwd.pdf"},
        library_status="enabled",
        invocation_id="inv-pdf-escape",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_SOURCE_NOT_FOUND
    assert "passwd" not in result.error_message
    assert "etc" not in result.error_message
    assert recorder.tx_entered == 0


# ─── AC-7: missing PDF file → SOURCE_NOT_FOUND ───────────────────────


def test_ac7_pdf_missing_file_becomes_source_not_found() -> None:
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    sandbox.read_bytes.side_effect = FileNotFoundError("nope.pdf")
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="x.md", title="x", body="x", cross_refs=[])]
        )
    )

    result = run_library_ingest(
        sandbox,
        {"source_type": "pdf", "source_ref": "nope.pdf"},
        library_status="enabled",
        invocation_id="inv-pdf-missing",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_SOURCE_NOT_FOUND
    assert "nope.pdf" not in result.error_message


# ─── AC-8: encrypted PDF → PDF_INVALID ───────────────────────────────


def test_ac8_encrypted_pdf_raises_pdf_invalid() -> None:
    from pypdf import PdfWriter

    # Build a tiny PDF and encrypt it with a password.
    writer = PdfWriter()
    writer.add_blank_page(width=612, height=792)
    writer.encrypt(user_password="secret", owner_password="ownersecret")
    buf = io.BytesIO()
    writer.write(buf)
    pdf_bytes = buf.getvalue()

    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="x.md", title="x", body="x", cross_refs=[])]
        )
    )

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "enc.pdf",
            "source_content": base64.b64encode(pdf_bytes).decode("ascii"),
        },
        library_status="enabled",
        invocation_id="inv-pdf-enc",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_PDF_INVALID
    assert "enc.pdf" not in result.error_message
    assert recorder.tx_entered == 0


# ─── AC-9: PDF branch reuses text/markdown writer code ──────────────


def test_ac9_pdf_and_text_paths_produce_identical_page_shape() -> None:
    """Given identical summarizer output, text and PDF produce the
    same on-disk byte layout for drafted pages and the same
    commit_metadata key-set modulo the pdf_partial_extraction flag."""
    drafts = [
        PageDraft(
            path="docs/alpha.md",
            title="Alpha",
            body="Alpha body.",
            # Story 13-11: cross-refs must resolve. ``index.md`` is
            # always a valid link target, so we use it here.
            cross_refs=["index.md"],
        ),
    ]

    # Text call.
    set_summarizer(_make_fake_summarizer(drafts, tokens_used=50))
    rec_text = _TxRecorder()
    sb_text = _make_sandbox_mock(rec_text)
    run_library_ingest(
        sb_text,
        {
            "source_type": "text",
            "source_ref": "source.txt",
            "source_content": "Hello text world.",
        },
        library_status="enabled",
        invocation_id="inv-t",
    )

    # PDF call with identical summarizer output.
    set_summarizer(_make_fake_summarizer(drafts, tokens_used=50))
    rec_pdf = _TxRecorder()
    sb_pdf = _make_sandbox_mock(rec_pdf)
    pdf_bytes = _build_text_pdf(["Hello pdf world."])
    run_library_ingest(
        sb_pdf,
        {
            "source_type": "pdf",
            "source_ref": "source.txt",  # same basename to match source_name
            "source_content": base64.b64encode(pdf_bytes).decode("ascii"),
        },
        library_status="enabled",
        invocation_id="inv-p",
    )

    # The drafted page content must be byte-identical.
    text_page = rec_text.files["docs/alpha.md"]
    pdf_page = rec_pdf.files["docs/alpha.md"]
    # Strip the ingest_date line (differs by wall clock second) before
    # comparing — every OTHER byte must match.
    def _strip_ingest_date(content: str) -> str:
        return "\n".join(
            line
            for line in content.splitlines()
            if not line.startswith("ingest_date:")
        )

    assert _strip_ingest_date(text_page) == _strip_ingest_date(pdf_page)
    # Same commit_metadata keys except the pdf flag.
    text_keys = set(rec_text.last_commit_metadata.keys())
    pdf_keys = set(rec_pdf.last_commit_metadata.keys())
    assert pdf_keys - text_keys == set()  # no pdf flag unless partial
    assert text_keys == pdf_keys


# ─── AC-10 / AC-11: base64 and raw-bytes source_content both accepted ─


def test_ac10_base64_source_content_accepted() -> None:
    pdf_bytes = _build_text_pdf(["B64 page."])
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="n.md", title="n", body="b", cross_refs=[])]
        )
    )
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "brief.pdf",
            "source_content": base64.b64encode(pdf_bytes).decode("ascii"),
        },
        library_status="enabled",
        invocation_id="inv-b64",
    )
    assert result.success is True, result.error_message
    sandbox.read_bytes.assert_not_called()


def test_ac11_raw_bytes_source_content_accepted() -> None:
    pdf_bytes = _build_text_pdf(["Raw page."])
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="n.md", title="n", body="b", cross_refs=[])]
        )
    )
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "brief.pdf",
            "source_content": pdf_bytes,  # raw bytes, not base64
        },
        library_status="enabled",
        invocation_id="inv-raw",
    )
    assert result.success is True, result.error_message


# ─── AC-12: run_library_ingest translates PDF errors into SkillResult ─


def test_ac12_empty_extraction_error_translates_to_skillresult() -> None:
    pdf_bytes = _build_empty_text_pdf()
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="x.md", title="x", body="x", cross_refs=[])]
        )
    )

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "scan.pdf",
            "source_content": base64.b64encode(pdf_bytes).decode("ascii"),
        },
        library_status="enabled",
        invocation_id="inv-12a",
        skill_name="library_ingest",
    )
    assert result.invocation_id == "inv-12a"
    assert result.skill_name == "library_ingest"
    assert result.error_code == LIBRARY_INGEST_PDF_EMPTY_EXTRACTION


def test_ac12_invalid_pdf_error_translates_to_skillresult() -> None:
    recorder = _TxRecorder()
    sandbox = _make_sandbox_mock(recorder)
    set_summarizer(
        _make_fake_summarizer(
            [PageDraft(path="x.md", title="x", body="x", cross_refs=[])]
        )
    )

    result = run_library_ingest(
        sandbox,
        {
            "source_type": "pdf",
            "source_ref": "bad.pdf",
            "source_content": base64.b64encode(b"not a pdf at all").decode(
                "ascii"
            ),
        },
        library_status="enabled",
        invocation_id="inv-12b",
    )
    assert result.invocation_id == "inv-12b"
    assert result.error_code == LIBRARY_INGEST_PDF_INVALID


# ─── read_bytes helper contract (new sandbox method) ────────────────


def test_library_sandbox_read_bytes_round_trip(tmp_path: Any) -> None:
    """Happy path + path escape for the new read_bytes helper."""
    root = tmp_path / "lib"
    root.mkdir()
    (root / "blob.bin").write_bytes(b"\x00\x01\x02\xff")
    sb = LibrarySandbox(str(root), git_enabled=False)
    assert sb.read_bytes("blob.bin") == b"\x00\x01\x02\xff"

    with pytest.raises(PathEscapeError):
        sb.read_bytes("../../etc/passwd")

    with pytest.raises(FileNotFoundError):
        sb.read_bytes("does-not-exist.bin")
