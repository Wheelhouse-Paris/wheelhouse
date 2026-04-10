"""Acceptance tests for the LibrarySandbox git layer (Story 13-4).

Covers FR14, NFR5, NFR17, NFR19, NFR22 and ADR-039 (commit message
format, author attribution, pre-commit checks). Every test in this file
maps to one of the AC-1..AC-12 BDDs in
``_bmad-output/implementation-artifacts/wh/13-4-commit-per-write-with-transactional-semantics.md``.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Iterator

import pytest

from wheelhouse.errors import (
    LibraryCommitError,
    LibraryDiskFullError,
    LibraryGitError,
    LibraryTransactionError,
    WheelhouseError,
)
from wheelhouse.skills.library_sandbox import (
    LibrarySandbox,
    TransactionHandle,
    _default_disk_space_check,
    _sanitize_git_stderr,
)


# ─── Skip the whole file if `git` is not on PATH ──────────────────────
pytestmark = pytest.mark.skipif(
    shutil.which("git") is None,
    reason="git CLI not available — required by LibrarySandbox git layer",
)


# ─── Fixtures ─────────────────────────────────────────────────────────


@pytest.fixture()
def library_root(tmp_path: Path) -> Path:
    root = tmp_path / "library"
    root.mkdir()
    return root


@pytest.fixture()
def isolated_git_env(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Iterator[None]:
    """Run tests with NO ambient git config.

    AC-3 requires that the sandbox set author/committer explicitly via
    env vars so a container with no ``~/.gitconfig`` still produces
    valid commits. We assert that by stripping every possible source of
    ambient identity.
    """
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    monkeypatch.delenv("GIT_AUTHOR_NAME", raising=False)
    monkeypatch.delenv("GIT_AUTHOR_EMAIL", raising=False)
    monkeypatch.delenv("GIT_COMMITTER_NAME", raising=False)
    monkeypatch.delenv("GIT_COMMITTER_EMAIL", raising=False)
    yield


@pytest.fixture()
def git_sandbox(
    library_root: Path, isolated_git_env: None
) -> LibrarySandbox:
    return LibrarySandbox(
        str(library_root), git_enabled=True, agent_name="alice"
    )


def _git_log_lines(root: Path, fmt: str) -> list[str]:
    out = subprocess.check_output(
        ["git", "log", f"--format={fmt}"], cwd=str(root), text=True
    )
    return [line for line in out.splitlines() if line]


def _git_log_count(root: Path) -> int:
    return len(_git_log_lines(root, "%H"))


# ─── AC-1: bare-on-demand git init ────────────────────────────────────


def test_no_git_dir_before_first_commit(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    # Sandbox construction must NOT touch the filesystem with git.
    assert not (library_root / ".git").exists()


def test_git_init_on_first_begin_commit(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "first commit")
    git_sandbox.write("a.md", "alpha")
    git_sandbox.commit(pages_created=["a.md"])
    assert (library_root / ".git").is_dir()
    assert _git_log_count(library_root) == 1


def test_git_init_is_idempotent(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    # First commit creates .git/.
    git_sandbox.begin("ingest", "first")
    git_sandbox.write("a.md", "x")
    git_sandbox.commit(pages_created=["a.md"])
    git_dir_inode_before = (library_root / ".git").stat().st_ino
    # Second begin/commit must reuse the same repo, not re-init.
    git_sandbox.begin("ingest", "second")
    git_sandbox.write("b.md", "y")
    git_sandbox.commit(pages_created=["b.md"])
    git_dir_inode_after = (library_root / ".git").stat().st_ino
    assert git_dir_inode_before == git_dir_inode_after
    assert _git_log_count(library_root) == 2


# ─── AC-2: single commit per skill invocation, structured message ─────


def test_single_commit_for_multiple_writes(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "Added 3 pages from client-brief.pdf")
    git_sandbox.write("a.md", "page a")
    git_sandbox.write("b.md", "page b")
    git_sandbox.write("c.md", "page c")
    git_sandbox.write("index.md", "updated index")
    git_sandbox.commit(
        sources=["client-brief.pdf"],
        pages_created=["a.md", "b.md", "c.md"],
        pages_updated=["index.md"],
    )
    assert _git_log_count(library_root) == 1
    # All four files appear in the single commit.
    files = subprocess.check_output(
        ["git", "log", "--name-only", "--pretty=format:"],
        cwd=str(library_root),
        text=True,
    ).split()
    assert set(files) == {"a.md", "b.md", "c.md", "index.md"}


def test_commit_message_structured_format(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "Added 3 pages from client-brief.pdf")
    git_sandbox.write("a.md", "a")
    git_sandbox.write("b.md", "b")
    git_sandbox.write("c.md", "c")
    git_sandbox.write("index.md", "i")
    git_sandbox.commit(
        sources=["client-brief.pdf"],
        pages_created=["a.md", "b.md", "c.md"],
        pages_updated=["index.md"],
    )
    full = subprocess.check_output(
        ["git", "log", "-1", "--format=%B"],
        cwd=str(library_root),
        text=True,
    )
    lines = full.strip().splitlines()
    assert lines[0] == "[ingest] Added 3 pages from client-brief.pdf"
    body = "\n".join(lines[1:])
    assert "Sources: client-brief.pdf" in body
    assert "Pages created: a.md, b.md, c.md" in body
    assert "Pages updated: index.md" in body


def test_commit_message_omits_empty_body_fields(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("lint", "no findings")
    git_sandbox.write("a.md", "a")
    git_sandbox.commit()  # all metadata None
    msg = subprocess.check_output(
        ["git", "log", "-1", "--format=%B"],
        cwd=str(library_root),
        text=True,
    )
    assert msg.strip() == "[lint] no findings"
    assert "Sources" not in msg
    assert "Pages created" not in msg


# ─── AC-3: explicit author / committer attribution ────────────────────


def test_commit_author_attribution_explicit(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "first")
    git_sandbox.write("a.md", "a")
    git_sandbox.commit(pages_created=["a.md"])
    line = subprocess.check_output(
        ["git", "log", "-1", "--format=%an|%ae|%cn|%ce"],
        cwd=str(library_root),
        text=True,
    ).strip()
    an, ae, cn, ce = line.split("|")
    assert an == "Agent alice"
    assert ae == "alice@wheelhouse.dev"
    assert cn == "Agent alice"
    assert ce == "alice@wheelhouse.dev"


# ─── AC-4: write-then-rollback leaves no partial state ────────────────


def test_rollback_removes_uncommitted_writes_after_initial_commit(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    # Establish HEAD with a baseline commit.
    git_sandbox.begin("ingest", "baseline")
    git_sandbox.write("base.md", "base")
    git_sandbox.commit(pages_created=["base.md"])

    # New txn with two writes, then rollback.
    git_sandbox.begin("ingest", "scratch")
    git_sandbox.write("a.md", "alpha")
    git_sandbox.write("b.md", "beta")
    git_sandbox.rollback()

    assert not (library_root / "a.md").exists()
    assert not (library_root / "b.md").exists()
    assert git_sandbox.exists("base.md") is True
    assert git_sandbox.exists("a.md") is False
    assert _git_log_count(library_root) == 1  # only baseline


# ─── AC-5: rollback before any commit (no HEAD yet) ───────────────────


def test_rollback_on_fresh_repo_before_first_commit(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "scratch")
    git_sandbox.write("a.md", "x")
    git_sandbox.rollback()
    assert not (library_root / "a.md").exists()
    # Repo is still in valid no-HEAD state. A new transaction must
    # succeed and produce the first commit.
    git_sandbox.begin("ingest", "first")
    git_sandbox.write("b.md", "y")
    git_sandbox.commit(pages_created=["b.md"])
    assert _git_log_count(library_root) == 1


# ─── AC-6: commit failure aborts cleanly ──────────────────────────────


def test_commit_failure_resets_working_tree(
    library_root: Path,
    git_sandbox: LibrarySandbox,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Baseline commit so HEAD exists.
    git_sandbox.begin("ingest", "baseline")
    git_sandbox.write("base.md", "base")
    git_sandbox.commit(pages_created=["base.md"])

    real_git = git_sandbox._git

    def boom_on_commit(args: list[str], **kw: object):  # type: ignore[no-untyped-def]
        if args and args[0] == "commit":
            raise LibraryCommitError("git commit failed: simulated")
        return real_git(args, **kw)  # type: ignore[arg-type]

    monkeypatch.setattr(git_sandbox, "_git", boom_on_commit)

    git_sandbox.begin("ingest", "doomed")
    git_sandbox.write("a.md", "x")
    git_sandbox.write("b.md", "y")
    with pytest.raises(LibraryCommitError):
        git_sandbox.commit(pages_created=["a.md", "b.md"])

    # Working tree reset, no new commit.
    monkeypatch.undo()
    assert not (library_root / "a.md").exists()
    assert not (library_root / "b.md").exists()
    assert _git_log_count(library_root) == 1
    # Transaction state cleared so a fresh begin() works.
    git_sandbox.begin("ingest", "after recovery")
    git_sandbox.write("c.md", "z")
    git_sandbox.commit(pages_created=["c.md"])


# ─── AC-7: pre-commit disk-space check ────────────────────────────────


def test_disk_full_check_aborts_commit(
    library_root: Path, isolated_git_env: None
) -> None:
    def always_full(_root: str) -> None:
        raise LibraryDiskFullError(required_bytes=10 * 1024 * 1024)

    sandbox = LibrarySandbox(
        str(library_root),
        git_enabled=True,
        agent_name="alice",
        disk_space_check=always_full,
    )
    sandbox.begin("ingest", "doomed")
    sandbox.write("a.md", "x")
    with pytest.raises(LibraryDiskFullError) as exc:
        sandbox.commit(pages_created=["a.md"])
    assert "Library is full" in str(exc.value)
    # Working tree reset.
    assert not (library_root / "a.md").exists()
    # No commit created (still no HEAD).
    head_check = subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD"],
        cwd=str(library_root),
        capture_output=True,
        text=True,
    )
    assert head_check.returncode != 0


def test_disk_full_error_message_actionable() -> None:
    err = LibraryDiskFullError()
    assert str(err).startswith("Library is full")
    assert "free disk space" in str(err)


def test_default_disk_space_check_passes_on_normal_volume(
    library_root: Path,
) -> None:
    # Sanity: regular tmp_path has more than 10 MB free.
    _default_disk_space_check(str(library_root))


# ─── AC-8: pre-commit hook injection point ────────────────────────────


def test_pre_commit_hook_invoked_with_staged_paths(
    library_root: Path, isolated_git_env: None
) -> None:
    seen: list[list[str]] = []

    def hook(paths: list[str]) -> None:
        seen.append(list(paths))

    sandbox = LibrarySandbox(
        str(library_root),
        git_enabled=True,
        agent_name="alice",
        pre_commit_hook=hook,
    )
    sandbox.begin("ingest", "with hook")
    sandbox.write("a.md", "a")
    sandbox.write("nested/b.md", "b")
    sandbox.commit(pages_created=["a.md", "nested/b.md"])
    assert seen == [["a.md", "nested/b.md"]]


def test_pre_commit_hook_failure_aborts_commit(
    library_root: Path, isolated_git_env: None
) -> None:
    class HookRejected(Exception):
        pass

    def hook(_paths: list[str]) -> None:
        raise HookRejected("dangling cross-reference")

    sandbox = LibrarySandbox(
        str(library_root),
        git_enabled=True,
        agent_name="alice",
        pre_commit_hook=hook,
    )
    sandbox.begin("ingest", "doomed")
    sandbox.write("a.md", "x")
    with pytest.raises(HookRejected):
        sandbox.commit(pages_created=["a.md"])
    # Working tree reset, no commit.
    assert not (library_root / "a.md").exists()
    head_check = subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD"],
        cwd=str(library_root),
        capture_output=True,
        text=True,
    )
    assert head_check.returncode != 0


# ─── AC-9: delete inside a transactional block ────────────────────────


def test_delete_is_included_in_commit(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "create old")
    git_sandbox.write("old.md", "stale")
    git_sandbox.commit(pages_created=["old.md"])

    git_sandbox.begin("lint", "Removed stale page")
    git_sandbox.delete("old.md")
    git_sandbox.commit(pages_updated=["old.md"])

    assert _git_log_count(library_root) == 2
    assert git_sandbox.exists("old.md") is False
    # The latest commit is a deletion.
    diff = subprocess.check_output(
        ["git", "show", "--name-status", "--format=", "HEAD"],
        cwd=str(library_root),
        text=True,
    ).strip()
    assert diff.startswith("D")
    assert "old.md" in diff


# ─── AC-10: backward compatibility (git_enabled=False) ────────────────


def test_non_git_mode_no_git_dir_created(library_root: Path) -> None:
    sandbox = LibrarySandbox(str(library_root))  # default git_enabled=False
    sandbox.write("a.md", "x")
    sandbox.delete("a.md")
    assert not (library_root / ".git").exists()


def test_commit_without_begin_raises_transaction_error(
    library_root: Path, isolated_git_env: None
) -> None:
    sandbox = LibrarySandbox(
        str(library_root), git_enabled=True, agent_name="alice"
    )
    with pytest.raises(LibraryTransactionError):
        sandbox.commit()


def test_commit_with_git_disabled_raises_transaction_error(
    library_root: Path,
) -> None:
    sandbox = LibrarySandbox(str(library_root))
    with pytest.raises(LibraryTransactionError):
        sandbox.commit()
    with pytest.raises(LibraryTransactionError):
        sandbox.rollback()
    with pytest.raises(LibraryTransactionError):
        sandbox.begin("ingest", "x")


def test_nested_begin_raises_transaction_error(
    git_sandbox: LibrarySandbox,
) -> None:
    git_sandbox.begin("ingest", "outer")
    with pytest.raises(LibraryTransactionError):
        git_sandbox.begin("ingest", "inner")


def test_begin_rejects_malformed_operation_or_summary(
    git_sandbox: LibrarySandbox,
) -> None:
    with pytest.raises(ValueError):
        git_sandbox.begin("", "summary")
    with pytest.raises(ValueError):
        git_sandbox.begin("ingest pull", "summary")
    with pytest.raises(ValueError):
        git_sandbox.begin("ingest", "")
    with pytest.raises(ValueError):
        git_sandbox.begin("ingest", "line1\nline2")


# ─── AC-11: commit performance on a 500-page repo (NFR5) ──────────────


def test_commit_under_5s_on_500_pages(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    # Bulk-create 500 small pages in a setup commit.
    git_sandbox.begin("ingest", "bulk seed")
    for i in range(500):
        git_sandbox.write(f"page-{i:04d}.md", f"content {i}\n")
    seed_pages = [f"page-{i:04d}.md" for i in range(500)]
    git_sandbox.commit(pages_created=seed_pages)

    # Now measure a single-page-add commit on the populated repo.
    start = time.monotonic()
    git_sandbox.begin("ingest", "one more")
    git_sandbox.write("page-extra.md", "extra")
    git_sandbox.commit(pages_created=["page-extra.md"])
    elapsed = time.monotonic() - start
    assert elapsed < 5.0, f"single-page commit took {elapsed:.2f}s (>5s NFR5)"


# ─── AC-12: context manager rollback / commit ─────────────────────────


def test_context_manager_commits_on_normal_exit(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    with git_sandbox.transaction("ingest", "ctx happy") as txn:
        assert isinstance(txn, TransactionHandle)
        git_sandbox.write("a.md", "a")
        txn.commit_metadata["pages_created"] = ["a.md"]
    assert _git_log_count(library_root) == 1
    msg = subprocess.check_output(
        ["git", "log", "-1", "--format=%B"], cwd=str(library_root), text=True
    )
    assert "Pages created: a.md" in msg


def test_context_manager_rolls_back_on_exception(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    class BlockFailed(Exception):
        pass

    with pytest.raises(BlockFailed):
        with git_sandbox.transaction("ingest", "ctx doomed"):
            git_sandbox.write("a.md", "x")
            raise BlockFailed()

    assert not (library_root / "a.md").exists()
    # Re-entering with a new transaction must work.
    with git_sandbox.transaction("ingest", "after recovery") as txn:
        git_sandbox.write("b.md", "y")
        txn.commit_metadata["pages_created"] = ["b.md"]
    assert _git_log_count(library_root) == 1


def test_transaction_metadata_accumulation(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    with git_sandbox.transaction("ingest", "accumulating") as txn:
        txn.commit_metadata.setdefault("pages_created", [])
        for name in ("a.md", "b.md"):
            git_sandbox.write(name, name)
            txn.commit_metadata["pages_created"].append(name)
        txn.commit_metadata["sources"] = ["doc.pdf"]
    msg = subprocess.check_output(
        ["git", "log", "-1", "--format=%B"], cwd=str(library_root), text=True
    )
    assert "Pages created: a.md, b.md" in msg
    assert "Sources: doc.pdf" in msg


# ─── Error sanitization (NFR9 parity) ─────────────────────────────────


def test_sanitize_git_stderr_strips_absolute_paths() -> None:
    raw = (
        "fatal: Unable to create '/workspace/.library/.git/index.lock': "
        "File exists"
    )
    cleaned = _sanitize_git_stderr(raw)
    assert "/workspace" not in cleaned
    assert "<path>" in cleaned
    # Tag tokens like "fatal:" are preserved.
    assert "fatal:" in cleaned


def test_commit_error_message_sanitized(
    library_root: Path,
    git_sandbox: LibrarySandbox,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    real_run = subprocess.run

    def fake_run(args, **kwargs):  # type: ignore[no-untyped-def]
        if isinstance(args, list) and len(args) >= 2 and args[0] == "git" and args[1] == "commit":
            # Use a non-lock failure here so this test exercises path
            # sanitization in isolation. Story 13-6 routes lock-collision
            # stderr through the retry wrapper into LibraryBusyError —
            # tested separately in test_library_sandbox_lock.py.
            return subprocess.CompletedProcess(
                args=args,
                returncode=128,
                stdout="",
                stderr=(
                    "fatal: bad object "
                    f"{library_root}/.git/objects/ab/cdef HEAD"
                ),
            )
        return real_run(args, **kwargs)

    monkeypatch.setattr(subprocess, "run", fake_run)

    git_sandbox.begin("ingest", "doomed")
    git_sandbox.write("a.md", "x")
    with pytest.raises(LibraryCommitError) as exc:
        git_sandbox.commit(pages_created=["a.md"])
    msg = str(exc.value)
    assert str(library_root) not in msg
    assert "<path>" in msg


# ─── Error class hierarchy sanity ─────────────────────────────────────


def test_error_classes_subclass_wheelhouse_error() -> None:
    assert issubclass(LibraryGitError, WheelhouseError)
    assert issubclass(LibraryCommitError, LibraryGitError)
    assert issubclass(LibraryDiskFullError, LibraryGitError)
    assert issubclass(LibraryTransactionError, LibraryGitError)


def test_errors_reexported_from_wheelhouse_package() -> None:
    import wheelhouse

    assert wheelhouse.LibraryGitError is LibraryGitError
    assert wheelhouse.LibraryCommitError is LibraryCommitError
    assert wheelhouse.LibraryDiskFullError is LibraryDiskFullError
    assert wheelhouse.LibraryTransactionError is LibraryTransactionError
