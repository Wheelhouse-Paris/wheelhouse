"""Acceptance tests for LibrarySandbox (Story 13-1).

These tests are written ATDD-first: they MUST fail until
`wheelhouse.skills.library_sandbox.LibrarySandbox` exists with the
semantics described in the story spec.

Covers FR38 (path canonicalization), NFR7 (sandbox restriction to root),
and NFR9 (no raw path leaks in error strings).
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from wheelhouse.errors import PathEscapeError  # noqa: F401 — imported for AC-2/6/7/8
from wheelhouse.skills.library_sandbox import LibrarySandbox


@pytest.fixture()
def library_root(tmp_path: Path) -> Path:
    root = tmp_path / "library"
    root.mkdir()
    (root / "notes").mkdir()
    (root / "notes" / "client.md").write_text("hello client")
    (root / "pages").mkdir()
    (root / "pages" / "b.md").write_text("page b")
    (root / "pages" / "c.md").write_text("page c")
    (root / "a.md").write_text("top a")
    return root


# ─── AC-1: canonical path inside root resolves and reads ─────────────────────
def test_read_inside_root(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    assert sandbox.read("notes/client.md") == "hello client"


# ─── AC-2: parent traversal rejected, no fs call, no raw path in error ───────
def test_read_parent_traversal_rejected(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises(PathEscapeError) as excinfo:
        sandbox.read("../../etc/passwd")
    msg = str(excinfo.value)
    assert "../" not in msg
    assert "etc/passwd" not in msg


# ─── AC-3: absolute path outside root rejected ───────────────────────────────
def test_read_absolute_path_outside_rejected(tmp_path: Path) -> None:
    lib_a = tmp_path / "lib_a"
    lib_a.mkdir()
    lib_b = tmp_path / "lib_b"
    lib_b.mkdir()
    (lib_b / "secret.md").write_text("top secret")
    sandbox = LibrarySandbox(str(lib_a))
    with pytest.raises(PathEscapeError):
        sandbox.read(str(lib_b / "secret.md"))


# ─── AC-4: symlink escape rejected via realpath ──────────────────────────────
def test_symlink_escape_rejected(tmp_path: Path) -> None:
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "hosts").write_text("127.0.0.1 localhost")
    lib = tmp_path / "lib"
    lib.mkdir()
    os.symlink(str(outside), str(lib / "escape"))
    sandbox = LibrarySandbox(str(lib))
    with pytest.raises(PathEscapeError):
        sandbox.read("escape/hosts")


# ─── AC-5: write inside root roundtrips ──────────────────────────────────────
def test_write_inside_root_roundtrip(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    sandbox.write("pages/alpha.md", "hello")
    assert (library_root / "pages" / "alpha.md").read_text() == "hello"
    assert sandbox.read("pages/alpha.md") == "hello"


# ─── AC-6: write escape creates no file ──────────────────────────────────────
def test_write_escape_creates_no_file(library_root: Path, tmp_path: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises(PathEscapeError):
        sandbox.write("../outside.md", "payload")
    assert not (library_root.parent / "outside.md").exists()


# ─── AC-7: list scoped to subdir; escape rejected ────────────────────────────
def test_list_scoped_to_subdir(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    result = sandbox.list("pages")
    # Paths returned relative to the Library root, sorted.
    assert "pages/b.md" in result
    assert "pages/c.md" in result
    assert "a.md" not in result


def test_list_escape_rejected(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises(PathEscapeError):
        sandbox.list("../")


# ─── AC-8: delete inside root; escape rejected ───────────────────────────────
def test_delete_inside_root(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    (library_root / "pages" / "tmp.md").write_text("tmp")
    sandbox.delete("pages/tmp.md")
    assert not (library_root / "pages" / "tmp.md").exists()


def test_delete_escape_rejected(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises(PathEscapeError):
        sandbox.delete("../etc/passwd")


# ─── AC-9: error string is sanitized (NFR9) ──────────────────────────────────
def test_path_escape_error_sanitized(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises(PathEscapeError) as excinfo:
        sandbox.read("../../../sensitive/secret.key")
    msg = str(excinfo.value)
    assert "sensitive/secret.key" not in msg
    assert "../" not in msg
    # Must be a short, generic, log-safe message.
    assert len(msg) < 200


def test_path_escape_error_debug_hash_present(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises(PathEscapeError) as excinfo:
        sandbox.read("../../nowhere")
    err = excinfo.value
    assert hasattr(err, "_debug_hash")
    # Short hex digest — no raw path.
    assert isinstance(err._debug_hash, str)
    assert all(c in "0123456789abcdef" for c in err._debug_hash)
    assert 8 <= len(err._debug_hash) <= 32


# ─── AC-10: library root canonicalized via symlink at construction ──────────
def test_root_canonicalized_via_symlink(tmp_path: Path) -> None:
    real = tmp_path / "real_root"
    real.mkdir()
    (real / "x.md").write_text("x")
    link = tmp_path / "link"
    os.symlink(str(real), str(link))
    sandbox = LibrarySandbox(str(link))
    # realpath should have resolved the symlink
    assert sandbox.read("x.md") == "x"


# ─── AC-11: non-existent root raises ─────────────────────────────────────────
def test_root_must_exist(tmp_path: Path) -> None:
    missing = tmp_path / "nope"
    with pytest.raises((FileNotFoundError, ValueError)):
        LibrarySandbox(str(missing))


# ─── AC-12: root must be a directory ─────────────────────────────────────────
def test_root_must_be_dir(tmp_path: Path) -> None:
    f = tmp_path / "file.txt"
    f.write_text("hi")
    with pytest.raises(ValueError):
        LibrarySandbox(str(f))


# ─── Sibling-prefix attack — guards against naive startswith() ───────────────
def test_sibling_prefix_attack(tmp_path: Path) -> None:
    lib_a = tmp_path / "libA"
    lib_a.mkdir()
    lib_a_evil = tmp_path / "libA-evil"
    lib_a_evil.mkdir()
    (lib_a_evil / "x.md").write_text("evil")
    sandbox = LibrarySandbox(str(lib_a))
    with pytest.raises(PathEscapeError):
        sandbox.read(str(lib_a_evil / "x.md"))


def test_empty_relpath_rejected(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises((PathEscapeError, ValueError)):
        sandbox.read("")


# ─── exists() — validation semantics same as read/write/delete ───────────────
def test_exists_inside_and_escape_rejected(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))
    assert sandbox.exists("notes/client.md") is True
    assert sandbox.exists("notes/does_not_exist.md") is False
    with pytest.raises(PathEscapeError):
        sandbox.exists("../outside")
