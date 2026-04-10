"""Acceptance tests for the LibrarySandbox crash-recovery sweep (Story 13-5).

Covers NFR18 (stale `.git/index.lock` >5min auto-removed), NFR19 (no
partial state on failure) and NFR23 (agent restart discards uncommitted
changes from a prior crashed lint). Every test in this file maps to one
of the AC-1..AC-12 BDDs in
``_bmad-output/implementation-artifacts/wh/13-5-crash-recovery-on-agent-restart.md``.
"""

from __future__ import annotations

import logging
import os
import shutil
import subprocess
from pathlib import Path
from typing import Iterator

import pytest

from wheelhouse.skills.library_sandbox import (
    _DEFAULT_STALE_LOCK_TIMEOUT_S,
    LibrarySandbox,
)


# ─── Skip the whole file if `git` is not on PATH ──────────────────────
pytestmark = pytest.mark.skipif(
    shutil.which("git") is None,
    reason="git CLI not available — required by LibrarySandbox crash recovery",
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
    """Strip ambient git identity so the sandbox env-var wiring is exercised."""
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    for var in (
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
    ):
        monkeypatch.delenv(var, raising=False)
    yield


class _SleepRecorder:
    """Mockable sleep that records calls and returns immediately."""

    def __init__(self) -> None:
        self.calls: list[float] = []

    def __call__(self, seconds: float) -> None:
        self.calls.append(seconds)


@pytest.fixture()
def sleep_recorder() -> _SleepRecorder:
    return _SleepRecorder()


@pytest.fixture()
def crash_sandbox(
    library_root: Path,
    isolated_git_env: None,
    sleep_recorder: _SleepRecorder,
    caplog: pytest.LogCaptureFixture,
) -> LibrarySandbox:
    caplog.set_level(logging.WARNING, logger="wheelhouse.library_sandbox")
    return LibrarySandbox(
        str(library_root),
        git_enabled=True,
        agent_name="alice",
        sleep=sleep_recorder,
    )


def _commit_baseline(sandbox: LibrarySandbox, rel: str, content: str) -> None:
    """Helper: commit ``rel`` with ``content`` through the sandbox transaction API."""
    sandbox.begin("seed", "baseline")
    sandbox.write(rel, content)
    sandbox.commit(pages_created=[rel])


def _git_status_porcelain(root: Path) -> str:
    return subprocess.check_output(
        ["git", "status", "--porcelain"], cwd=str(root), text=True
    )


def _git_head_short(root: Path) -> str:
    return subprocess.check_output(
        ["git", "rev-parse", "--short", "HEAD"], cwd=str(root), text=True
    ).strip()


def _init_fresh_repo_no_commits(sandbox: LibrarySandbox) -> None:
    """Force `.git/` initialization without producing any commits."""
    sandbox.begin("seed", "no-op")
    sandbox.rollback()


# ─── AC-1: clean-repo recovery is a no-op ─────────────────────────────


def test_recover_on_clean_repo_is_noop(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    caplog: pytest.LogCaptureFixture,
) -> None:
    _commit_baseline(crash_sandbox, "page.md", "alpha")
    head_before = _git_head_short(library_root)
    porcelain_before = _git_status_porcelain(library_root)
    assert porcelain_before == ""

    caplog.clear()
    crash_sandbox.recover_from_crash()

    assert _git_head_short(library_root) == head_before
    assert _git_status_porcelain(library_root) == ""
    # No "Discarded" warning, no "lock" warning.
    msgs = [r.message for r in caplog.records]
    assert not any("Discarded" in m for m in msgs)
    assert not any("index.lock" in m for m in msgs)


# ─── AC-2: discard uncommitted modification (NFR23) ────────────────────


def test_recover_discards_uncommitted_modification(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    caplog: pytest.LogCaptureFixture,
) -> None:
    _commit_baseline(crash_sandbox, "page.md", "alpha")
    short_hash = _git_head_short(library_root)
    # Simulate a crash mid-write — content changes on disk but no commit.
    (library_root / "page.md").write_text("BETA-CRASHED")
    assert _git_status_porcelain(library_root) != ""

    caplog.clear()
    crash_sandbox.recover_from_crash()

    assert (library_root / "page.md").read_text() == "alpha"
    assert _git_status_porcelain(library_root) == ""
    msgs = [r.getMessage() for r in caplog.records]
    assert any("Discarded" in m and short_hash in m for m in msgs), msgs


# ─── AC-3: discard untracked file ──────────────────────────────────────


def test_recover_discards_untracked_file(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    caplog: pytest.LogCaptureFixture,
) -> None:
    _commit_baseline(crash_sandbox, "index.md", "i")
    (library_root / "orphan.md").write_text("from a crashed write")
    assert (library_root / "orphan.md").exists()

    caplog.clear()
    crash_sandbox.recover_from_crash()

    assert not (library_root / "orphan.md").exists()
    assert _git_status_porcelain(library_root) == ""
    msgs = [r.getMessage() for r in caplog.records]
    assert any("Discarded" in m for m in msgs)


# ─── AC-4: recovery on a fresh repo with no HEAD yet ───────────────────


def test_recover_on_fresh_repo_no_head(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    caplog: pytest.LogCaptureFixture,
) -> None:
    _init_fresh_repo_no_commits(crash_sandbox)
    assert (library_root / ".git").is_dir()
    # No HEAD yet — verify git rev-parse fails before recovery.
    assert (
        subprocess.run(
            ["git", "rev-parse", "--verify", "HEAD"],
            cwd=str(library_root),
            capture_output=True,
        ).returncode
        != 0
    )
    (library_root / "draft.md").write_text("from a crashed first ingest")

    caplog.clear()
    crash_sandbox.recover_from_crash()

    assert not (library_root / "draft.md").exists()
    msgs = [r.getMessage() for r in caplog.records]
    assert any("no commits yet" in m for m in msgs), msgs

    # A subsequent commit must still work — the repo remains valid.
    crash_sandbox.begin("ingest", "first real commit")
    crash_sandbox.write("real.md", "real content")
    crash_sandbox.commit(pages_created=["real.md"])
    assert _git_head_short(library_root) != ""


# ─── AC-5: stale `.git/index.lock` (>5min) removed immediately ─────────


def test_recover_removes_stale_lock_immediately(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    sleep_recorder: _SleepRecorder,
    caplog: pytest.LogCaptureFixture,
) -> None:
    _commit_baseline(crash_sandbox, "page.md", "alpha")
    lock = library_root / ".git" / "index.lock"
    lock.write_text("")  # touch
    # Backdate the lock 10 minutes — well past the 5-minute default.
    stale_mtime = lock.stat().st_mtime - 600
    os.utime(lock, (stale_mtime, stale_mtime))

    caplog.clear()
    crash_sandbox.recover_from_crash()

    assert not lock.exists()
    # Stale lock — no waiting allowed.
    assert sleep_recorder.calls == []
    msgs = [r.getMessage() for r in caplog.records]
    assert any("stale" in m and "index.lock" in m for m in msgs), msgs


# ─── AC-6: fresh `.git/index.lock` (≤5min) waits then removes ─────────


def test_recover_waits_for_fresh_lock_then_removes(
    library_root: Path,
    isolated_git_env: None,
    sleep_recorder: _SleepRecorder,
    caplog: pytest.LogCaptureFixture,
) -> None:
    caplog.set_level(logging.WARNING, logger="wheelhouse.library_sandbox")
    sandbox = LibrarySandbox(
        str(library_root),
        git_enabled=True,
        agent_name="alice",
        sleep=sleep_recorder,
        stale_lock_timeout_s=5.0,
    )
    _commit_baseline(sandbox, "page.md", "alpha")
    lock = library_root / ".git" / "index.lock"
    lock.write_text("")
    fresh_mtime = lock.stat().st_mtime - 1  # 1 second old
    os.utime(lock, (fresh_mtime, fresh_mtime))

    caplog.clear()
    sandbox.recover_from_crash()

    assert not lock.exists()
    # Slept once for the remaining (~4s) of the 5s budget.
    assert len(sleep_recorder.calls) == 1
    waited = sleep_recorder.calls[0]
    assert 3.0 <= waited <= 5.0, f"unexpected wait: {waited}"
    msgs = [r.getMessage() for r in caplog.records]
    assert any("index.lock" in m for m in msgs)


# ─── AC-7: no lock present → no lock-handling warning ─────────────────


def test_recover_no_lock_no_warning(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    caplog: pytest.LogCaptureFixture,
) -> None:
    _commit_baseline(crash_sandbox, "page.md", "alpha")
    assert not (library_root / ".git" / "index.lock").exists()

    caplog.clear()
    crash_sandbox.recover_from_crash()

    msgs = [r.getMessage() for r in caplog.records]
    assert not any("index.lock" in m for m in msgs)


# ─── AC-8: no-op when git_enabled=False ───────────────────────────────


def test_recover_noop_when_git_disabled(
    library_root: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Make ANY subprocess.run from inside library_sandbox blow up loudly.
    real_run = subprocess.run

    def loud_run(*args: object, **kwargs: object) -> object:
        raise AssertionError(
            "subprocess.run must NOT be called when git_enabled=False"
        )

    monkeypatch.setattr(
        "wheelhouse.skills.library_sandbox.subprocess.run", loud_run
    )

    sandbox = LibrarySandbox(str(library_root), git_enabled=False)
    sandbox.recover_from_crash()  # must return silently

    # Restore so teardown is happy.
    monkeypatch.setattr(
        "wheelhouse.skills.library_sandbox.subprocess.run", real_run
    )

    assert not (library_root / ".git").exists()


# ─── AC-9: recovery does not open or touch a transaction ──────────────


def test_recover_does_not_open_transaction(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
) -> None:
    assert crash_sandbox._txn is None
    _commit_baseline(crash_sandbox, "page.md", "alpha")
    assert crash_sandbox._txn is None

    crash_sandbox.recover_from_crash()
    assert crash_sandbox._txn is None

    # A subsequent transaction works.
    crash_sandbox.begin("ingest", "after recovery")
    crash_sandbox.write("after.md", "x")
    crash_sandbox.commit(pages_created=["after.md"])
    assert (library_root / "after.md").read_text() == "x"


# ─── AC-10: foreign .git/ directory + git_enabled=False → no-op ───────


def test_recover_with_foreign_dot_git_when_disabled(
    library_root: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Create a sub-directory literally named .git that is NOT a git repo.
    (library_root / ".git").mkdir()
    (library_root / ".git" / "definitely-not-real-git").write_text("nope")

    def loud_run(*args: object, **kwargs: object) -> object:
        raise AssertionError(
            "subprocess.run must NOT be called when git_enabled=False"
        )

    monkeypatch.setattr(
        "wheelhouse.skills.library_sandbox.subprocess.run", loud_run
    )

    sandbox = LibrarySandbox(str(library_root), git_enabled=False)
    sandbox.recover_from_crash()  # must return silently

    monkeypatch.setattr(
        "wheelhouse.skills.library_sandbox.subprocess.run", subprocess.run
    )


# ─── AC-11: TOCTOU race — lock disappears between stat and unlink ─────


def test_recover_handles_lock_disappearing_race(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _commit_baseline(crash_sandbox, "page.md", "alpha")
    lock = library_root / ".git" / "index.lock"
    lock.write_text("")
    stale_mtime = lock.stat().st_mtime - 600
    os.utime(lock, (stale_mtime, stale_mtime))

    real_unlink = os.unlink
    raised = {"once": False}

    def flaky_unlink(path: str) -> None:
        if not raised["once"] and str(path).endswith("index.lock"):
            raised["once"] = True
            # Remove for real, then raise to simulate "another process won".
            real_unlink(path)
            raise FileNotFoundError(path)
        return real_unlink(path)

    monkeypatch.setattr(
        "wheelhouse.skills.library_sandbox.os.unlink", flaky_unlink
    )

    # Must not propagate the FileNotFoundError.
    crash_sandbox.recover_from_crash()
    assert not lock.exists()
    assert raised["once"] is True


# ─── AC-12: reset modified-and-untracked in one call ──────────────────


def test_recover_resets_modified_and_untracked_in_one_call(
    crash_sandbox: LibrarySandbox,
    library_root: Path,
    caplog: pytest.LogCaptureFixture,
) -> None:
    _commit_baseline(crash_sandbox, "a.md", "alpha")
    (library_root / "a.md").write_text("MODIFIED")
    (library_root / "b.md").write_text("UNTRACKED")
    assert _git_status_porcelain(library_root) != ""

    caplog.clear()
    crash_sandbox.recover_from_crash()

    assert (library_root / "a.md").read_text() == "alpha"
    assert not (library_root / "b.md").exists()
    assert _git_status_porcelain(library_root) == ""
    discard_msgs = [
        (r.getMessage())
        for r in caplog.records
        if "Discarded" in (r.getMessage())
    ]
    assert len(discard_msgs) == 1


# ─── Sanity: shared 5-minute constant exposed for Story 13-6 ──────────


def test_default_stale_lock_timeout_constant() -> None:
    assert _DEFAULT_STALE_LOCK_TIMEOUT_S == 300.0
