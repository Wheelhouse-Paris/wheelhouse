"""Tests for the LibrarySandbox boot wiring + library dispatch interception.

Story 13-7 — AC-2..AC-5 (boot-path) and AC-10/AC-11 (dispatch-path).
"""

from __future__ import annotations

import logging
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock

import pytest

from wheelhouse.skills.library_sandbox import LibrarySandbox
from wheelhouse.types import SkillInvocation, SkillProgress, SkillResult

from agent_claude import library as library_mod
from agent_claude.library import build_library_sandbox


# ─── AC-5: Disabled status → no sandbox constructed ───────────────────


def test_build_library_sandbox_disabled_returns_none(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    """AC-5: library_status=disabled → no LibrarySandbox constructed,
    no git command invoked, returns None with an INFO log."""
    ctor_mock = MagicMock()
    monkeypatch.setattr(library_mod, "LibrarySandbox", ctor_mock)

    config = {
        "library_status": "disabled",
        "agent_name": "alice",
    }
    with caplog.at_level(logging.INFO, logger="agent_claude"):
        result = build_library_sandbox(
            config, library_root=str(tmp_path / "library")
        )

    assert result is None
    ctor_mock.assert_not_called()
    assert any(
        "Library disabled" in record.message for record in caplog.records
    )


# ─── AC-2 + AC-3: enabled → one sandbox + one recover_from_crash ──────


def test_build_library_sandbox_enabled_constructs_once_and_recovers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """AC-2 + AC-3: library_status=enabled → constructs LibrarySandbox
    exactly once with git_enabled=True and calls recover_from_crash()
    exactly once before returning."""
    library_root = tmp_path / "library"
    # Root does not exist — build_library_sandbox must create it.
    assert not library_root.exists()

    fake_instance = MagicMock(name="LibrarySandboxInstance")
    fake_instance.recover_from_crash = MagicMock()
    ctor_mock = MagicMock(return_value=fake_instance)
    monkeypatch.setattr(library_mod, "LibrarySandbox", ctor_mock)

    config = {"library_status": "enabled", "agent_name": "alice"}
    result = build_library_sandbox(config, library_root=str(library_root))

    assert result is fake_instance
    # Constructor called exactly once (AC-2).
    assert ctor_mock.call_count == 1
    call_args = ctor_mock.call_args
    assert call_args.kwargs["git_enabled"] is True
    assert call_args.kwargs["agent_name"] == "alice"
    assert call_args.args[0] == str(library_root)
    # Root directory now exists (AC-2 — must be created if missing).
    assert library_root.is_dir()
    # recover_from_crash called exactly once (AC-3).
    fake_instance.recover_from_crash.assert_called_once()
    # No transaction method called during boot (AC-3 last clause).
    fake_instance.begin.assert_not_called()
    fake_instance.commit.assert_not_called()
    fake_instance.rollback.assert_not_called()
    fake_instance.transaction.assert_not_called()


def test_build_library_sandbox_creates_missing_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """AC-2 last clause: the Library root is created if it did not
    exist. Separate assertion from the happy-path test so a future
    refactor cannot accidentally keep one without the other."""
    deep = tmp_path / "a" / "b" / "c" / "library"
    assert not deep.exists()

    fake_instance = MagicMock()
    fake_instance.recover_from_crash = MagicMock()
    monkeypatch.setattr(
        library_mod, "LibrarySandbox", MagicMock(return_value=fake_instance)
    )

    build_library_sandbox(
        {"library_status": "enabled", "agent_name": "bob"},
        library_root=str(deep),
    )
    assert deep.is_dir()


# ─── AC-4: Recovery failure degrades gracefully ───────────────────────


def test_build_library_sandbox_recovery_failure_returns_none(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    """AC-4: recover_from_crash() raising LibraryGitError must NOT
    propagate — the function logs a WARNING and returns None so the
    agent continues booting."""
    from wheelhouse.errors import LibraryCommitError

    fake_instance = MagicMock()
    fake_instance.recover_from_crash = MagicMock(
        side_effect=LibraryCommitError("git reset failed: disk I/O")
    )
    monkeypatch.setattr(
        library_mod, "LibrarySandbox", MagicMock(return_value=fake_instance)
    )

    config = {"library_status": "enabled", "agent_name": "alice"}
    with caplog.at_level(logging.WARNING, logger="agent_claude"):
        result = build_library_sandbox(
            config, library_root=str(tmp_path / "library")
        )

    assert result is None
    # A WARNING was emitted and it mentions recovery.
    warning_messages = [
        r.message for r in caplog.records if r.levelno == logging.WARNING
    ]
    assert any(
        "Library recovery failed" in msg for msg in warning_messages
    ), f"expected recovery failure warning, got: {warning_messages}"


# ─── AC-5 read-only sub-case: sandbox still constructed ───────────────


def test_build_library_sandbox_read_only_still_constructs_and_recovers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """AC-5 read-only clause: read-only status DOES construct a
    sandbox (future 13-14/13-16 read paths depend on a recovered repo)
    — only the per-invocation handler refuses writes."""
    fake_instance = MagicMock()
    fake_instance.recover_from_crash = MagicMock()
    ctor = MagicMock(return_value=fake_instance)
    monkeypatch.setattr(library_mod, "LibrarySandbox", ctor)

    config = {"library_status": "read-only", "agent_name": "alice"}
    result = build_library_sandbox(
        config, library_root=str(tmp_path / "library")
    )

    assert result is fake_instance
    ctor.assert_called_once()
    assert ctor.call_args.kwargs["git_enabled"] is True
    fake_instance.recover_from_crash.assert_called_once()


# ─── Guardrail: missing config keys degrade cleanly ───────────────────


# ─── AC-10 / AC-11: dispatch interception ─────────────────────────────
#
# These tests drive `_handle_library_skill_invocation` directly — they do
# not exercise the full `_handle_skill_invocation` fork path because
# ``test_loop.py`` has pre-existing failures unrelated to this story
# (stale SkillInvocation field names on the legacy test fixtures). The
# unit under test for AC-10/AC-11 is the Library dispatch helper and
# the registry lookup gate, both of which are exercised end-to-end here.


async def test_handle_library_skill_invocation_does_not_call_claude(
    tmp_path: Path,
) -> None:
    """AC-10: dispatch routes library_ingest to the handler, NOT Claude.

    The helper publishes SkillProgress first (preserving the 5-4 ack
    contract) and then a SkillResult produced by run_library_ingest —
    no Claude API call is possible because the helper does not accept
    a claude_client argument.
    """
    from agent_claude.loop import _handle_library_skill_invocation

    connection = MagicMock()
    connection.publish = AsyncMock()

    sandbox_mock = MagicMock(spec=LibrarySandbox)
    config: dict[str, object] = {
        "library_sandbox": sandbox_mock,
        "library_status": "enabled",
    }

    msg = SkillInvocation(
        skill_name="library_ingest",
        agent_id="alice",
        invocation_id="inv-42",
        parameters={"source_type": "text", "source_ref": "foo.txt"},
    )

    await _handle_library_skill_invocation(
        msg, connection, "main", config=config
    )

    # Exactly two publishes: SkillProgress then SkillResult.
    assert connection.publish.call_count == 2
    first = connection.publish.call_args_list[0].args[1]
    second = connection.publish.call_args_list[1].args[1]
    assert isinstance(first, SkillProgress)
    assert first.invocation_id == "inv-42"
    assert isinstance(second, SkillResult)
    assert second.invocation_id == "inv-42"
    assert second.skill_name == "library_ingest"
    # Scaffold sentinel — 13-7 always produces NOT_IMPLEMENTED on the
    # post-validation path.
    assert second.success is False
    assert second.error_code == "LIBRARY_INGEST_NOT_IMPLEMENTED"
    # Sandbox transactional surface never touched by scaffolding.
    sandbox_mock.begin.assert_not_called()
    sandbox_mock.commit.assert_not_called()
    sandbox_mock.transaction.assert_not_called()


async def test_handle_library_skill_invocation_degrades_when_config_none(
    tmp_path: Path,
) -> None:
    """AC-8 safety net: calling the dispatch helper with no config dict
    produces a clean LIBRARY_DISABLED refusal instead of crashing."""
    from agent_claude.loop import _handle_library_skill_invocation

    connection = MagicMock()
    connection.publish = AsyncMock()

    msg = SkillInvocation(
        skill_name="library_ingest",
        agent_id="alice",
        invocation_id="inv-99",
        parameters={"source_type": "text", "source_ref": "foo"},
    )

    await _handle_library_skill_invocation(
        msg, connection, "main", config=None
    )

    # Two publishes still: SkillProgress + SkillResult(LIBRARY_DISABLED).
    assert connection.publish.call_count == 2
    result = connection.publish.call_args_list[1].args[1]
    assert isinstance(result, SkillResult)
    assert result.success is False
    assert result.error_code == "LIBRARY_DISABLED"


def test_library_skill_registry_is_importable_from_loop() -> None:
    """AC-1 sanity: the loop module imports the registry by name, so a
    rename in library_ingest.py breaks both sites at once.

    Story 13-18 added ``library_lint`` as a second registered entry —
    this test pins that the loop-side alias sees the new handler without
    any loop.py code change (AC-10 of 13-18).
    """
    from agent_claude.loop import LIBRARY_SKILL_REGISTRY
    from wheelhouse.skills.library_lint import run_library_lint

    assert "library_ingest" in LIBRARY_SKILL_REGISTRY
    assert "library_lint" in LIBRARY_SKILL_REGISTRY
    assert LIBRARY_SKILL_REGISTRY["library_lint"] is run_library_lint


def test_build_library_sandbox_missing_library_status_treated_as_disabled(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """If run_startup never set library_status (shouldn't happen, but
    defensive), the function must not crash — it should treat it as
    disabled and return None."""
    ctor = MagicMock()
    monkeypatch.setattr(library_mod, "LibrarySandbox", ctor)

    result = build_library_sandbox(
        {"agent_name": "alice"}, library_root=str(tmp_path / "library")
    )
    assert result is None
    ctor.assert_not_called()
