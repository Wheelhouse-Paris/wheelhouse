"""Acceptance tests for LibrarySandbox concurrent-write serialization (Story 13-6).

Covers FR22, NFR18 and ADR-040 (git index.lock retry protocol with
5-minute stale detection). Every test in this file maps to one of the
AC-1..AC-11 BDDs in
``_bmad-output/implementation-artifacts/wh/13-6-concurrent-write-serialization-via-git-index-lock.md``.
"""

from __future__ import annotations

import logging
import os
import shutil
import subprocess
import threading
import time
from pathlib import Path
from typing import Any, Callable, Iterator

import pytest

from wheelhouse.errors import (
    LibraryBusyError,
    LibraryCommitError,
    LibraryGitError,
    WheelhouseError,
)
from wheelhouse.skills import library_sandbox as ls_module
from wheelhouse.skills.library_sandbox import (
    LibrarySandbox,
    _is_lock_collision,
    _LOCK_MAX_ATTEMPTS,
    _LOCK_RETRY_WAIT_S,
    _LOCK_STALE_AFTER_S,
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


# ─── Test helpers ─────────────────────────────────────────────────────


_LOCK_STDERR = (
    "fatal: Unable to create '/workspace/.library/.git/index.lock': File exists.\n"
    "Another git process seems to be running in this repository, e.g."
)


class _CollidingGit:
    """Stand-in for `_git` that emits configurable lock collisions.

    Each entry in ``script`` is either:
      - a string ``"OK"``        → return a successful CompletedProcess
      - a string ``"LOCK"``      → raise LibraryCommitError with lock stderr
      - a string ``"BOOM:<msg>"`` → raise LibraryCommitError with arbitrary stderr
    """

    def __init__(self, script: list[str]) -> None:
        self.script = list(script)
        self.calls: list[list[str]] = []

    def __call__(
        self,
        args: list[str],
        *,
        check: bool = True,
        suppress_stderr: bool = False,
    ) -> Any:
        # Read-only diagnostics like `rev-parse --verify HEAD` MUST NOT
        # consume script entries — story 13-6 AC-6 explicitly excludes
        # them from the lock-retry path, and rollback() calls _has_head
        # internally.
        if args and args[0] == "rev-parse":
            # HEAD exists in tests that bootstrap a real commit first.
            return subprocess.CompletedProcess(
                args=["git", *args], returncode=0, stdout="", stderr=""
            )
        self.calls.append(list(args))
        if not self.script:
            return subprocess.CompletedProcess(
                args=["git", *args], returncode=0, stdout="", stderr=""
            )
        step = self.script.pop(0)
        if step == "OK":
            return subprocess.CompletedProcess(
                args=["git", *args], returncode=0, stdout="", stderr=""
            )
        if step == "LOCK":
            raise LibraryCommitError(f"git add failed: {_LOCK_STDERR}")
        if step.startswith("BOOM:"):
            msg = step[len("BOOM:"):]
            raise LibraryCommitError(f"git add failed: {msg}")
        raise AssertionError(f"unknown script step: {step!r}")


class _FakeSleep:
    def __init__(self) -> None:
        self.calls: list[float] = []

    def __call__(self, secs: float) -> None:
        self.calls.append(secs)


# ─── Pure unit tests on _is_lock_collision ────────────────────────────


def test_is_lock_collision_true_on_index_lock_stderr() -> None:
    assert _is_lock_collision(_LOCK_STDERR) is True


def test_is_lock_collision_false_on_other_lock_files() -> None:
    assert (
        _is_lock_collision("fatal: Unable to create 'HEAD.lock': File exists.")
        is False
    )
    assert (
        _is_lock_collision(
            "fatal: Unable to create 'packed-refs.lock': File exists."
        )
        is False
    )


def test_is_lock_collision_false_on_unrelated_error() -> None:
    assert _is_lock_collision("fatal: bad object HEAD") is False
    assert _is_lock_collision("") is False
    assert _is_lock_collision(None) is False  # type: ignore[arg-type]


# ─── LibraryBusyError shape (Task 1) ──────────────────────────────────


def test_library_busy_error_subclasses_library_git_and_wheelhouse() -> None:
    err = LibraryBusyError()
    assert isinstance(err, LibraryGitError)
    assert isinstance(err, WheelhouseError)
    assert str(err).startswith("Library is busy")
    assert err.code == "LIBRARY_BUSY"


# ─── AC-1: uncontested → no retry, no warning ─────────────────────────


def test_uncontested_commit_no_retry_no_warning(
    git_sandbox: LibrarySandbox,
    caplog: pytest.LogCaptureFixture,
) -> None:
    fake_sleep = _FakeSleep()
    git_sandbox._sleep = fake_sleep  # type: ignore[method-assign]
    with caplog.at_level(logging.WARNING, logger="wheelhouse.library_sandbox"):
        git_sandbox.begin("ingest", "first page")
        git_sandbox.write("a.md", "alpha")
        git_sandbox.commit(pages_created=["a.md"])
    assert fake_sleep.calls == []
    assert not any("Library is busy" in r.message for r in caplog.records)
    assert not any("stale Library lock removed" in r.message for r in caplog.records)


# ─── AC-2: contended → released → success on retry ───────────────────


def test_contended_then_released_succeeds(
    git_sandbox: LibrarySandbox,
) -> None:
    # Bootstrap a real repo so HEAD exists.
    git_sandbox.begin("ingest", "bootstrap")
    git_sandbox.write("seed.md", "seed")
    git_sandbox.commit(pages_created=["seed.md"])

    fake_sleep = _FakeSleep()
    git_sandbox._sleep = fake_sleep  # type: ignore[method-assign]

    # Force the "fresh lock, no stale removal" branch so the retry path
    # consumes a sleep instead of taking the bonus stale-removal retry.
    git_sandbox._remove_stale_lock_if_present = lambda: False  # type: ignore[method-assign]

    # Replace the bare _git seam with a script: collide once, succeed.
    fake = _CollidingGit(script=["LOCK", "OK", "OK"])
    git_sandbox._git = fake  # type: ignore[method-assign]

    git_sandbox._git_with_lock_retry(["add", "--", "x.md"])
    # 1 collision → 1 sleep, then 1 success
    assert fake_sleep.calls == [_LOCK_RETRY_WAIT_S]


# ─── AC-3: stale lock auto-removed with warning ───────────────────────


def test_stale_lock_removed_with_warning(
    library_root: Path,
    git_sandbox: LibrarySandbox,
    caplog: pytest.LogCaptureFixture,
) -> None:
    # Bootstrap so .git exists and HEAD is set.
    git_sandbox.begin("ingest", "bootstrap")
    git_sandbox.write("seed.md", "seed")
    git_sandbox.commit(pages_created=["seed.md"])

    # Plant a stale index.lock (mtime > 5 min ago).
    lock = library_root / ".git" / "index.lock"
    lock.write_text("")
    stale_mtime = time.time() - (_LOCK_STALE_AFTER_S + 60)
    os.utime(str(lock), (stale_mtime, stale_mtime))

    fake_sleep = _FakeSleep()
    git_sandbox._sleep = fake_sleep  # type: ignore[method-assign]
    fake = _CollidingGit(script=["LOCK", "OK"])
    git_sandbox._git = fake  # type: ignore[method-assign]

    with caplog.at_level(logging.WARNING, logger="wheelhouse.library_sandbox"):
        git_sandbox._git_with_lock_retry(["add", "--", "x.md"])

    assert any(
        "stale Library lock removed" in r.message for r in caplog.records
    )
    # Stale removal does NOT consume a sleep.
    assert fake_sleep.calls == []
    # Lock file was removed.
    assert not lock.exists()


# ─── AC-4: persistent contention → LibraryBusyError after 3 attempts ─


def test_persistent_contention_raises_busy_after_three_attempts(
    git_sandbox: LibrarySandbox,
) -> None:
    git_sandbox.begin("ingest", "bootstrap")
    git_sandbox.write("seed.md", "seed")
    git_sandbox.commit(pages_created=["seed.md"])

    fake_sleep = _FakeSleep()
    git_sandbox._sleep = fake_sleep  # type: ignore[method-assign]

    # Always collide; lock is not stale (mtime = now means age <= 0 < 300s).
    fake = _CollidingGit(script=["LOCK"] * 10)
    git_sandbox._git = fake  # type: ignore[method-assign]

    # Stub stale check to always say "fresh" (no removal).
    git_sandbox._remove_stale_lock_if_present = lambda: False  # type: ignore[method-assign]

    with pytest.raises(LibraryBusyError) as exc_info:
        git_sandbox._git_with_lock_retry(["add", "--", "x.md"])

    assert str(exc_info.value).startswith("Library is busy")
    # 3 attempts → 2 sleeps between them.
    assert len(fake_sleep.calls) == _LOCK_MAX_ATTEMPTS - 1
    assert all(s == _LOCK_RETRY_WAIT_S for s in fake_sleep.calls)


def test_busy_failure_clears_active_transaction(
    git_sandbox: LibrarySandbox,
) -> None:
    """After LibraryBusyError, _txn must be None and a new transaction must start cleanly."""
    git_sandbox.begin("ingest", "bootstrap")
    git_sandbox.write("seed.md", "seed")
    git_sandbox.commit(pages_created=["seed.md"])

    git_sandbox._sleep = _FakeSleep()  # type: ignore[method-assign]
    git_sandbox._remove_stale_lock_if_present = lambda: False  # type: ignore[method-assign]

    git_sandbox.begin("ingest", "doomed")
    git_sandbox.write("doomed.md", "x")
    # Force commit to take the busy path.
    git_sandbox._git = _CollidingGit(script=["LOCK"] * 10)  # type: ignore[method-assign]
    with pytest.raises(LibraryBusyError):
        git_sandbox.commit(pages_created=["doomed.md"])

    assert git_sandbox._txn is None
    # A new transaction with the *real* git must succeed.
    # Restore the real _git method.
    del git_sandbox._git  # type: ignore[attr-defined]
    git_sandbox.begin("ingest", "next")
    git_sandbox.write("after.md", "y")
    git_sandbox.commit(pages_created=["after.md"])


# ─── AC-5: lock-namespace isolation ───────────────────────────────────


def test_no_wh_directory_after_full_transaction(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "iso")
    git_sandbox.write("a.md", "alpha")
    git_sandbox.commit(pages_created=["a.md"])
    entries = set(os.listdir(str(library_root)))
    assert ".wh" not in entries
    # workspace.lock is in .wh/, never under the Library root.
    assert "workspace.lock" not in entries


# ─── AC-6: read-only _has_head bypasses retry wrapper ────────────────


def test_has_head_does_not_use_retry_wrapper(
    git_sandbox: LibrarySandbox,
) -> None:
    git_sandbox.begin("ingest", "boot")
    git_sandbox.write("seed.md", "x")
    git_sandbox.commit(pages_created=["seed.md"])

    calls: list[list[str]] = []
    real = git_sandbox._git_with_lock_retry

    def spy(args: list[str]) -> Any:
        calls.append(list(args))
        return real(args)

    git_sandbox._git_with_lock_retry = spy  # type: ignore[method-assign]
    assert git_sandbox._has_head() is True
    # The spy must NOT have been called by _has_head.
    assert calls == []


# ─── AC-7: stale lock disappears between detection and removal ───────


def test_stale_lock_disappears_during_unlink(
    library_root: Path,
    git_sandbox: LibrarySandbox,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    git_sandbox.begin("ingest", "bootstrap")
    git_sandbox.write("seed.md", "seed")
    git_sandbox.commit(pages_created=["seed.md"])

    lock = library_root / ".git" / "index.lock"
    lock.write_text("")
    stale_mtime = time.time() - (_LOCK_STALE_AFTER_S + 60)
    os.utime(str(lock), (stale_mtime, stale_mtime))

    real_unlink = os.unlink

    def racy_unlink(path: str) -> None:
        # Simulate another process removing the lock first.
        if str(path).endswith("index.lock"):
            raise FileNotFoundError(path)
        real_unlink(path)

    monkeypatch.setattr(os, "unlink", racy_unlink)
    # Should NOT raise.
    result = git_sandbox._remove_stale_lock_if_present()
    assert result is True


def test_lock_age_seconds_returns_none_when_absent(
    library_root: Path, git_sandbox: LibrarySandbox
) -> None:
    git_sandbox.begin("ingest", "boot")
    git_sandbox.write("seed.md", "x")
    git_sandbox.commit(pages_created=["seed.md"])
    # No lock file present.
    assert git_sandbox._lock_age_seconds() is None


# ─── AC-8: busy error message has no path ────────────────────────────


def test_busy_error_message_has_no_filesystem_path() -> None:
    msg = str(LibraryBusyError())
    # Reject any "/" — the message is path-free by construction.
    assert "/" not in msg
    # Reject the lock file name even without a leading slash.
    assert "index.lock" not in msg
    assert msg.startswith("Library is busy")


# ─── AC-9: non-lock commit error passes through unchanged ────────────


def test_non_lock_commit_error_passes_through(
    git_sandbox: LibrarySandbox,
) -> None:
    git_sandbox.begin("ingest", "bootstrap")
    git_sandbox.write("seed.md", "seed")
    git_sandbox.commit(pages_created=["seed.md"])

    git_sandbox._sleep = _FakeSleep()  # type: ignore[method-assign]
    fake = _CollidingGit(script=["BOOM:fatal: index file corrupt"])
    git_sandbox._git = fake  # type: ignore[method-assign]

    with pytest.raises(LibraryCommitError) as exc_info:
        git_sandbox._git_with_lock_retry(["add", "--", "x.md"])
    # Original LibraryCommitError, NOT LibraryBusyError.
    assert not isinstance(exc_info.value, LibraryBusyError)
    assert "index file corrupt" in str(exc_info.value)
    # No retries — exactly one git call.
    assert len(fake.calls) == 1


# ─── AC-10: two-thread end-to-end serialization (real git) ───────────


def test_two_threads_serialize_end_to_end(
    library_root: Path, isolated_git_env: None
) -> None:
    sandbox = LibrarySandbox(
        str(library_root), git_enabled=True, agent_name="alice"
    )
    # Bootstrap so HEAD exists.
    sandbox.begin("ingest", "bootstrap")
    sandbox.write("seed.md", "seed")
    sandbox.commit(pages_created=["seed.md"])

    barrier = threading.Barrier(2)
    errors: list[BaseException] = []

    def worker(name: str) -> None:
        try:
            local = LibrarySandbox(
                str(library_root), git_enabled=True, agent_name=name
            )
            barrier.wait()
            local.begin("ingest", f"page from {name}")
            local.write(f"{name}.md", f"hello from {name}")
            local.commit(pages_created=[f"{name}.md"])
        except BaseException as exc:  # noqa: BLE001
            errors.append(exc)

    t1 = threading.Thread(target=worker, args=("one",))
    t2 = threading.Thread(target=worker, args=("two",))
    t1.start()
    t2.start()
    t1.join()
    t2.join()

    assert errors == [], f"thread errors: {errors!r}"
    # Both files exist on disk.
    assert (library_root / "one.md").exists()
    assert (library_root / "two.md").exists()
    # Three commits in linear history (seed + one + two), no merge.
    out = subprocess.check_output(
        ["git", "log", "--format=%H %P"],
        cwd=str(library_root),
        text=True,
    ).splitlines()
    assert len(out) == 3
    for line in out:
        parents = line.split()[1:]
        assert len(parents) <= 1, f"unexpected merge commit: {line!r}"


# ─── AC-11: uncontested overhead microbench (slow) ───────────────────


@pytest.mark.slow
def test_lock_overhead_microbench(
    git_sandbox: LibrarySandbox,
) -> None:
    """Smoke microbench: 10 commits should complete in well under 5s.

    The retry-loop overhead is one extra try-frame and one stderr-regex
    miss per uncontested call. We don't pin a hard ms budget here —
    just guard against catastrophic regression.
    """
    start = time.monotonic()
    for i in range(10):
        git_sandbox.begin("ingest", f"page {i}")
        git_sandbox.write(f"p{i}.md", f"content {i}")
        git_sandbox.commit(pages_created=[f"p{i}.md"])
    elapsed = time.monotonic() - start
    assert elapsed < 5.0, f"10 uncontested commits took {elapsed:.2f}s"
