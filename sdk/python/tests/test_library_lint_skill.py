"""Acceptance tests for the on-demand lint skill (Story 13-18).

Covers AC-1..AC-9 from
``_bmad-output/implementation-artifacts/wh/13-18-on-demand-lint-via-conversation.md``.

AC-10 (loop dispatch routing) lives in
``agent-claude/tests/test_library_boot.py`` because it imports the
``agent_claude`` package.

These tests use real ``tmp_path`` git-enabled LibrarySandbox fixtures
so the full-vs-incremental dispatch (AC-5) exercises the same marker
persistence stack as 13-17.
"""

from __future__ import annotations

import shutil
from pathlib import Path
from typing import Iterator, Sequence
from unittest.mock import patch

import pytest

from wheelhouse.skills import library_lint as lint_mod
from wheelhouse.skills.library_lint import (
    ACCEPTED_MODES,
    DEFAULT_MODE,
    LIBRARY_DISABLED,
    LIBRARY_LINT_INVALID_ARGS,
    LINT_STATE_PATH,
    LibraryPage,
    LintFinding,
    LintState,
    OPTIONAL_PARAMS,
    REQUIRED_PARAMS,
    SEVERITY_WARN,
    SKILL_NAME,
    CATEGORY_ORPHAN,
    CATEGORY_STALE_REF,
    get_lint_llm,
    run_library_lint,
    save_lint_state,
    set_lint_llm,
)
from wheelhouse.skills.library_ingest import SKILL_REGISTRY
from wheelhouse.skills.library_sandbox import LibrarySandbox


pytestmark = pytest.mark.skipif(
    shutil.which("git") is None,
    reason="git CLI not available — required by library_lint skill tests",
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


@pytest.fixture()
def seeded_sandbox(git_sandbox: LibrarySandbox) -> LibrarySandbox:
    """Sandbox with one committed page so git HEAD exists."""
    git_sandbox.begin("ingest", "seed")
    git_sandbox.write("a.md", "---\ncross_refs: []\n---\n# a\n")
    git_sandbox.commit(pages_created=["a.md"])
    return git_sandbox


@pytest.fixture(autouse=True)
def _clear_lint_llm() -> Iterator[None]:
    """Ensure the module-level LLM seam is always ``None`` at test start."""
    set_lint_llm(None)
    yield
    set_lint_llm(None)


# ─── AC-1: Skill identity constants ──────────────────────────────────


def test_skill_identity_constants() -> None:
    assert SKILL_NAME == "library_lint"
    assert REQUIRED_PARAMS == ()
    assert OPTIONAL_PARAMS == ("mode",)
    assert ACCEPTED_MODES == ("full", "incremental")
    assert DEFAULT_MODE == "incremental"

    exported = set(lint_mod.__all__)
    for name in ("SKILL_NAME", "REQUIRED_PARAMS", "OPTIONAL_PARAMS",
                 "ACCEPTED_MODES", "run_library_lint",
                 "set_lint_llm", "get_lint_llm"):
        assert name in exported, f"{name} missing from __all__"


# ─── AC-2: Disabled refusal ──────────────────────────────────────────


def test_disabled_when_status_is_disabled() -> None:
    result = run_library_lint(
        None,
        {},
        library_status="disabled",
        invocation_id="inv-1",
        skill_name="library_lint",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_DISABLED
    assert result.invocation_id == "inv-1"
    assert result.skill_name == "library_lint"
    # NFR9: no path echoed in error message.
    assert "/" not in result.error_message


def test_disabled_when_sandbox_is_none_even_with_enabled_status() -> None:
    result = run_library_lint(
        None,
        {},
        library_status="enabled",
        invocation_id="inv-1b",
        skill_name="library_lint",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_DISABLED


# ─── AC-3: Invalid mode rejected ─────────────────────────────────────


def test_invalid_mode_is_rejected(seeded_sandbox: LibrarySandbox) -> None:
    with patch.object(lint_mod, "lint_library") as full_spy, \
         patch.object(lint_mod, "lint_library_incremental") as inc_spy:
        result = run_library_lint(
            seeded_sandbox,
            {"mode": "deep-scan"},
            library_status="enabled",
            invocation_id="inv-2",
            skill_name="library_lint",
        )
    assert result.success is False
    assert result.error_code == LIBRARY_LINT_INVALID_ARGS
    # Echoes the rejected value and the accepted set.
    assert "deep-scan" in result.error_message
    assert "full" in result.error_message
    assert "incremental" in result.error_message
    # No detectors were run.
    full_spy.assert_not_called()
    inc_spy.assert_not_called()


# ─── AC-4: Default mode is "incremental" ─────────────────────────────


def test_default_mode_is_incremental(seeded_sandbox: LibrarySandbox) -> None:
    with patch.object(
        lint_mod, "lint_library_incremental", return_value=[]
    ) as inc_spy, patch.object(lint_mod, "lint_library") as full_spy:
        result = run_library_lint(
            seeded_sandbox,
            {},
            library_status="enabled",
            invocation_id="inv-3",
            skill_name="library_lint",
        )
    assert result.success is True
    assert inc_spy.call_count == 1
    full_spy.assert_not_called()
    # Sandbox is the first positional arg.
    assert inc_spy.call_args.args[0] is seeded_sandbox


# ─── AC-5: mode="full" bypasses the incremental marker ───────────────


def test_full_mode_leaves_marker_untouched(
    seeded_sandbox: LibrarySandbox, library_root: Path
) -> None:
    # Pre-seed a marker — we'll assert it is untouched after run.
    save_lint_state(
        seeded_sandbox,
        LintState(last_commit_sha="prevsha", last_run_at="2026-04-10T00:00:00Z"),
    )
    marker_before = (library_root / LINT_STATE_PATH).read_bytes()

    with patch.object(
        lint_mod, "lint_library", return_value=[]
    ) as full_spy, patch.object(
        lint_mod, "lint_library_incremental"
    ) as inc_spy:
        result = run_library_lint(
            seeded_sandbox,
            {"mode": "full"},
            library_status="enabled",
            invocation_id="inv-4",
            skill_name="library_lint",
        )

    assert result.success is True
    inc_spy.assert_not_called()
    full_spy.assert_called_once()
    # page_filter=None signals a full lint.
    assert full_spy.call_args.kwargs.get("page_filter") is None

    marker_after = (library_root / LINT_STATE_PATH).read_bytes()
    assert marker_before == marker_after


# ─── AC-6: Output summarizes findings with a category breakdown ──────


def test_output_summary_includes_counts_per_category(
    seeded_sandbox: LibrarySandbox,
) -> None:
    findings = [
        LintFinding(
            category=CATEGORY_ORPHAN, page="foo.md",
            detail="...", severity=SEVERITY_WARN,
        ),
        LintFinding(
            category=CATEGORY_STALE_REF, page="bar.md",
            detail="...", severity=SEVERITY_WARN,
        ),
    ]
    with patch.object(
        lint_mod, "lint_library_incremental", return_value=findings
    ):
        result = run_library_lint(
            seeded_sandbox,
            {},
            library_status="enabled",
            invocation_id="inv-5",
            skill_name="library_lint",
        )
    assert result.success is True
    assert "2" in result.output
    assert "orphan" in result.output
    assert "stale_ref" in result.output
    assert result.library_page_count == 2
    assert result.library_tokens == 0


# ─── AC-7: Empty findings list is reported as a clean pass ───────────


def test_empty_findings_reported_as_clean(
    seeded_sandbox: LibrarySandbox,
) -> None:
    with patch.object(lint_mod, "lint_library_incremental", return_value=[]):
        result = run_library_lint(
            seeded_sandbox,
            {},
            library_status="enabled",
            invocation_id="inv-6",
            skill_name="library_lint",
        )
    assert result.success is True
    assert "no findings" in result.output.lower()
    assert result.library_page_count == 0


# ─── AC-8: LLM-detector seam is honored ──────────────────────────────


def test_llm_seam_is_passed_through(
    seeded_sandbox: LibrarySandbox,
) -> None:
    def fake_llm(pages: Sequence[LibraryPage], prompt: str) -> list[dict]:
        return []

    set_lint_llm(fake_llm)
    try:
        with patch.object(
            lint_mod, "lint_library", return_value=[]
        ) as full_spy:
            result = run_library_lint(
                seeded_sandbox,
                {"mode": "full"},
                library_status="enabled",
                invocation_id="inv-7",
                skill_name="library_lint",
            )
        assert result.success is True
        # lint_library was called with llm_fn == installed fake.
        assert full_spy.call_args.kwargs.get("llm_fn") is fake_llm
        # The seam is NOT cleared after the call.
        assert get_lint_llm() is fake_llm
    finally:
        set_lint_llm(None)


# ─── AC-9: library_lint is registered in SKILL_REGISTRY ──────────────


def test_library_lint_is_registered() -> None:
    assert "library_lint" in SKILL_REGISTRY
    assert SKILL_REGISTRY["library_lint"] is run_library_lint
    # 13-7's registration is preserved.
    assert "library_ingest" in SKILL_REGISTRY


# ─── Defensive: detector exception becomes a clean SkillResult ───────


def test_detector_exception_is_caught_and_reported() -> None:
    from unittest.mock import MagicMock

    sandbox = MagicMock(spec=LibrarySandbox)
    with patch.object(
        lint_mod,
        "lint_library_incremental",
        side_effect=RuntimeError("boom"),
    ):
        result = run_library_lint(
            sandbox,
            {},
            library_status="enabled",
            invocation_id="inv-boom",
            skill_name="library_lint",
        )
    assert result.success is False
    assert result.error_code == LIBRARY_LINT_INVALID_ARGS
    # NFR9: upstream exception string is NOT propagated verbatim.
    assert "boom" not in result.error_message
