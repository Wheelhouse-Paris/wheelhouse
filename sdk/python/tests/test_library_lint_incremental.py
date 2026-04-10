"""Acceptance tests for incremental lint mode (Story 13-17).

Covers AC-1..AC-8 from
``_bmad-output/implementation-artifacts/wh/13-17-incremental-lint-mode.md``.

Unlike the pure-detection tests in ``test_library_lint_detection.py``,
this file exercises the full persistence stack: real ``tmp_path``
LibrarySandbox with ``git_enabled=True``, real ``git`` subprocess calls,
real JSON round-tripping. That is deliberate — the whole point of the
incremental path is that it composes with the 13-4 transaction layer
and the git history, so a pure-mock test would not catch regressions
in how the marker file is committed or how ``git diff`` is invoked.
"""

from __future__ import annotations

import dataclasses
import json
import shutil
import subprocess
from pathlib import Path
from typing import Iterator, Sequence
from unittest.mock import patch

import pytest

from wheelhouse.skills import library_lint as lint_mod
from wheelhouse.skills.library_lint import (
    LINT_STATE_PATH,
    LibraryPage,
    LintFinding,
    LintState,
    compute_changed_pages,
    lint_library,
    lint_library_incremental,
    load_lint_state,
    save_lint_state,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox


pytestmark = pytest.mark.skipif(
    shutil.which("git") is None,
    reason="git CLI not available — required by incremental lint mode",
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


def _page_body(cross_refs: Sequence[str] = ()) -> str:
    refs_yaml = "[" + ", ".join(cross_refs) + "]"
    return f"---\ncross_refs: {refs_yaml}\n---\n# body\n"


def _commit_page(
    sandbox: LibrarySandbox, path: str, content: str, summary: str
) -> None:
    sandbox.begin("ingest", summary)
    sandbox.write(path, content)
    sandbox.commit(pages_created=[path])


def _head_sha(root: Path) -> str:
    return subprocess.check_output(
        ["git", "rev-parse", "--verify", "HEAD"], cwd=str(root), text=True
    ).strip()


# ─── AC-1: LintState is a frozen value object ─────────────────────────


def test_lint_state_is_a_frozen_dataclass() -> None:
    s = LintState(last_commit_sha="abc123", last_run_at="2026-04-10T12:00:00Z")
    assert s.last_commit_sha == "abc123"
    assert s.last_run_at == "2026-04-10T12:00:00Z"
    with pytest.raises(dataclasses.FrozenInstanceError):
        s.last_commit_sha = "zzz"  # type: ignore[misc]


# ─── AC-2: load returns None on first run ─────────────────────────────


def test_load_lint_state_returns_none_when_marker_absent(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    # Needs at least an initialized repo so sandbox.exists works cleanly.
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")
    assert load_lint_state(git_sandbox) is None
    assert not (library_root / LINT_STATE_PATH).exists()


def test_load_lint_state_returns_none_on_malformed_marker(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")
    # Write a junk marker directly and commit it so the sandbox sees it.
    git_sandbox.begin("lint", "bogus marker")
    git_sandbox.write(LINT_STATE_PATH, "{not-json")
    git_sandbox.commit(pages_updated=[LINT_STATE_PATH])
    assert load_lint_state(git_sandbox) is None


# ─── AC-3: save persists through the transaction layer ───────────────


def test_save_lint_state_commits_marker_file(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")
    state = LintState(
        last_commit_sha="deadbeef", last_run_at="2026-04-10T12:34:56Z"
    )
    save_lint_state(git_sandbox, state)

    marker = library_root / LINT_STATE_PATH
    assert marker.exists()
    parsed = json.loads(marker.read_text())
    assert parsed == {
        "last_commit_sha": "deadbeef",
        "last_run_at": "2026-04-10T12:34:56Z",
    }

    # Verify the marker landed in a "[lint] ..." commit.
    subj = subprocess.check_output(
        ["git", "log", "-1", "--format=%s"], cwd=str(library_root), text=True
    ).strip()
    assert subj.startswith("[lint] ")

    # Round-trip.
    reloaded = load_lint_state(git_sandbox)
    assert reloaded == state


# ─── AC-4: compute_changed_pages returns *.md diff between shas ──────


def test_compute_changed_pages_returns_modified_and_added_md(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")
    _commit_page(git_sandbox, "b.md", _page_body(), "add b")
    sha_a = _head_sha(library_root)

    # Commit B: modify b.md, add c.md.
    git_sandbox.begin("ingest", "B")
    git_sandbox.write("b.md", _page_body(["a.md"]))
    git_sandbox.write("c.md", _page_body())
    git_sandbox.commit(
        pages_created=["c.md"], pages_updated=["b.md"]
    )

    changed = compute_changed_pages(git_sandbox, since_sha=sha_a)
    assert changed == {"b.md", "c.md"}
    # Non-page files (none in this scenario) are filtered out.
    assert LINT_STATE_PATH not in changed


def test_compute_changed_pages_filters_marker_file(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")
    sha_a = _head_sha(library_root)
    # Now write the marker file — this is a non-.md change.
    save_lint_state(
        git_sandbox,
        LintState(last_commit_sha=sha_a, last_run_at="2026-04-10T00:00:00Z"),
    )
    changed = compute_changed_pages(git_sandbox, since_sha=sha_a)
    # The marker file is not a page — it must not appear in the filter.
    assert LINT_STATE_PATH not in changed
    assert changed == set()


# ─── AC-5: HEAD == since_sha → empty set ─────────────────────────────


def test_compute_changed_pages_empty_when_head_equals_since(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")
    sha = _head_sha(library_root)
    assert compute_changed_pages(git_sandbox, since_sha=sha) == set()


# ─── AC-6: first run → full lint + persist marker ────────────────────


def test_incremental_first_run_is_full_lint_and_saves_marker(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "index.md", _page_body(["a.md"]), "index")
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")

    captured: dict = {}

    real_lint_library = lint_mod.lint_library

    def _spy_lint(sandbox, llm_fn=None, page_filter=None):
        captured["page_filter"] = page_filter
        return real_lint_library(sandbox, llm_fn=llm_fn, page_filter=page_filter)

    with patch.object(lint_mod, "lint_library", side_effect=_spy_lint) as m:
        findings = lint_library_incremental(git_sandbox, llm_fn=None)

    assert m.called
    assert captured["page_filter"] is None  # full lint on first run
    assert isinstance(findings, list)

    # Marker now exists and pins to the HEAD at the time of the call.
    # HEAD has since advanced because save_lint_state adds one commit,
    # so the marker should equal the sha BEFORE that marker commit — or
    # the sha AFTER, depending on when we sampled. The implementation
    # samples HEAD before saving, so the marker points at the last page
    # commit, not at the marker-write commit itself.
    marker = load_lint_state(git_sandbox)
    assert marker is not None
    # The recorded sha must be a valid ancestor of HEAD.
    rc = subprocess.run(
        ["git", "merge-base", "--is-ancestor", marker.last_commit_sha, "HEAD"],
        cwd=str(library_root),
    )
    assert rc.returncode == 0


# ─── AC-7: subsequent run restricts to changed pages ─────────────────


def test_incremental_second_run_uses_changed_page_filter(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "index.md", _page_body(["a.md", "b.md"]), "index")
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")
    _commit_page(git_sandbox, "b.md", _page_body(), "add b")

    # First run: seeds the marker via the real code path.
    lint_library_incremental(git_sandbox, llm_fn=None)
    state_after_first = load_lint_state(git_sandbox)
    assert state_after_first is not None

    # Modify b.md only.
    git_sandbox.begin("ingest", "edit b")
    git_sandbox.write("b.md", _page_body(["a.md"]))
    git_sandbox.commit(pages_updated=["b.md"])

    captured: dict = {}
    real_lint_library = lint_mod.lint_library

    def _spy_lint(sandbox, llm_fn=None, page_filter=None):
        captured["page_filter"] = (
            set(page_filter) if page_filter is not None else None
        )
        return real_lint_library(sandbox, llm_fn=llm_fn, page_filter=page_filter)

    with patch.object(lint_mod, "lint_library", side_effect=_spy_lint):
        lint_library_incremental(git_sandbox, llm_fn=None)

    assert captured["page_filter"] is not None
    assert "b.md" in captured["page_filter"]
    # The marker must NOT forward the untouched a.md into the filter.
    assert "a.md" not in captured["page_filter"]

    # Marker advanced.
    state_after_second = load_lint_state(git_sandbox)
    assert state_after_second is not None
    assert state_after_second.last_commit_sha != state_after_first.last_commit_sha


# ─── AC-8: nothing changed → short-circuit, no detection ─────────────


def test_incremental_short_circuits_when_nothing_changed(
    git_sandbox: LibrarySandbox, library_root: Path
) -> None:
    _commit_page(git_sandbox, "index.md", _page_body(["a.md"]), "index")
    _commit_page(git_sandbox, "a.md", _page_body(), "add a")

    # First run seeds the marker.
    lint_library_incremental(git_sandbox, llm_fn=None)
    state_before = load_lint_state(git_sandbox)
    assert state_before is not None

    # Second run: no page changes at all. lint_library MUST NOT be called.
    with patch.object(lint_mod, "lint_library") as m_lint:
        findings = lint_library_incremental(git_sandbox, llm_fn=None)
        assert not m_lint.called

    assert findings == []
    # Marker is untouched.
    state_after = load_lint_state(git_sandbox)
    assert state_after == state_before
