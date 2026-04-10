"""Unit tests for the library_ingest skill scaffolding (Story 13-7).

Every test in this file covers an AC in
``_bmad-output/implementation-artifacts/wh/13-7-library-ingest-skill-scaffolding.md``.

AC-12 in particular: every test uses a ``MagicMock`` sandbox double — no
real git, no filesystem I/O, no subprocess calls. The scaffold-only
nature of 13-7 makes that easy: ``run_library_ingest`` never touches
``self._git`` and the ``_ingest_pipeline`` delegate raises
``NOT_IMPLEMENTED`` before any transaction is opened.
"""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

from wheelhouse.errors import LibrarySkillError
from wheelhouse.skills import library_ingest as ingest_mod
from wheelhouse.skills.library_ingest import (
    ACCEPTED_SOURCE_TYPES,
    LIBRARY_DISABLED,
    LIBRARY_INGEST_INVALID_ARGS,
    LIBRARY_INGEST_NOT_IMPLEMENTED,
    LIBRARY_READ_ONLY,
    REQUIRED_PARAMS,
    SKILL_NAME,
    SKILL_REGISTRY,
    run_library_ingest,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox
from wheelhouse.types import SkillResult


# ─── Fixtures ─────────────────────────────────────────────────────────


@pytest.fixture()
def sandbox_mock() -> MagicMock:
    """A LibrarySandbox test double with every method recorded."""
    return MagicMock(spec=LibrarySandbox)


@pytest.fixture()
def valid_params() -> dict[str, str]:
    return {
        "source_type": "text",
        "source_ref": "brief.txt",
        "user_summary_hint": "client onboarding notes",
    }


# ─── AC-1: Registry discoverability ───────────────────────────────────


def test_skill_registry_lists_library_ingest() -> None:
    assert SKILL_NAME == "library_ingest"
    assert SKILL_NAME in SKILL_REGISTRY
    assert SKILL_REGISTRY[SKILL_NAME] is run_library_ingest
    # Required params: {"source_type", "source_ref"} (user_summary_hint
    # is optional and lives in OPTIONAL_PARAMS).
    assert set(REQUIRED_PARAMS) == {"source_type", "source_ref"}


def test_skill_registry_is_a_mapping_with_single_entry() -> None:
    """13-7 registers exactly one Library skill. 13-14/13-16 will add more."""
    assert list(SKILL_REGISTRY.keys()) == [SKILL_NAME]


# ─── AC-8: Disabled refusal ───────────────────────────────────────────


def test_run_library_ingest_disabled_when_status_is_disabled(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
) -> None:
    result = run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="disabled",
        invocation_id="inv-001",
    )
    assert isinstance(result, SkillResult)
    assert result.success is False
    assert result.error_code == LIBRARY_DISABLED
    assert "disabled" in result.error_message.lower()
    # Disabled path must not touch the sandbox at all.
    sandbox_mock.begin.assert_not_called()
    sandbox_mock.write.assert_not_called()
    sandbox_mock.commit.assert_not_called()


def test_run_library_ingest_disabled_when_sandbox_is_none(
    valid_params: dict[str, str],
) -> None:
    """AC-8: if the boot path returned None, the handler still produces a
    clean SkillResult — no AttributeError crashing the dispatch."""
    result = run_library_ingest(
        None,
        valid_params,
        library_status="enabled",
        invocation_id="inv-002",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_DISABLED


# ─── AC-6: Argument validation ────────────────────────────────────────


def test_run_library_ingest_missing_source_type(
    sandbox_mock: MagicMock,
) -> None:
    result = run_library_ingest(
        sandbox_mock,
        {},
        library_status="enabled",
        invocation_id="inv-003",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INVALID_ARGS
    assert "source_type" in result.error_message
    sandbox_mock.begin.assert_not_called()


def test_run_library_ingest_missing_source_ref(
    sandbox_mock: MagicMock,
) -> None:
    result = run_library_ingest(
        sandbox_mock,
        {"source_type": "text"},
        library_status="enabled",
        invocation_id="inv-004",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INVALID_ARGS
    assert "source_ref" in result.error_message


def test_run_library_ingest_error_message_does_not_echo_values(
    sandbox_mock: MagicMock,
) -> None:
    """NFR9: missing-parameter error names KEYS, never VALUES.

    The handler must not echo a parameter value back because
    ``source_ref`` can be a filesystem path (``/home/alice/secret.md``)
    and the SkillResult.error_message is a cloud-log path.
    """
    secret = "/home/alice/private/secret.md"
    result = run_library_ingest(
        sandbox_mock,
        {"source_ref": secret},  # source_type missing
        library_status="enabled",
        invocation_id="inv-005",
    )
    assert secret not in result.error_message
    assert "source_type" in result.error_message


def test_run_library_ingest_invalid_source_type(
    sandbox_mock: MagicMock,
) -> None:
    result = run_library_ingest(
        sandbox_mock,
        {"source_type": "xml", "source_ref": "foo"},
        library_status="enabled",
        invocation_id="inv-006",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_INVALID_ARGS
    assert "xml" in result.error_message
    # The error message lists every accepted value.
    for accepted in ACCEPTED_SOURCE_TYPES:
        assert accepted in result.error_message


# ─── AC-7: Read-only refusal (FR37) ───────────────────────────────────


_FR37_VERBATIM = (
    "Your Library is in read-only mode. Upgrade to Pro to add new sources."
)


def test_run_library_ingest_read_only_refusal_verbatim(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
) -> None:
    result = run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="read-only",
        invocation_id="inv-007",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_READ_ONLY
    # Exact string match — cloud-side copy mirrors this.
    assert result.error_message == _FR37_VERBATIM


def test_run_library_ingest_read_only_does_not_touch_sandbox(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
) -> None:
    run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="read-only",
        invocation_id="inv-008",
    )
    # Sandbox gate: no git call made.
    sandbox_mock.begin.assert_not_called()
    sandbox_mock.write.assert_not_called()
    sandbox_mock.commit.assert_not_called()
    sandbox_mock.rollback.assert_not_called()
    sandbox_mock.transaction.assert_not_called()


# ─── AC-9: Scaffold NOT_IMPLEMENTED sentinel ──────────────────────────


def test_run_library_ingest_happy_path_scaffold_sentinel(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
) -> None:
    result = run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="enabled",
        invocation_id="inv-009",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_NOT_IMPLEMENTED
    assert "13-8" in result.error_message
    assert "13-9" in result.error_message
    # Validation passed but no transaction was opened — the
    # scaffold never touches the sandbox.
    sandbox_mock.begin.assert_not_called()
    sandbox_mock.transaction.assert_not_called()


def test_run_library_ingest_echoes_invocation_id_and_skill_name(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
) -> None:
    result = run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="enabled",
        invocation_id="inv-010",
        skill_name="library_ingest",
    )
    assert result.invocation_id == "inv-010"
    assert result.skill_name == "library_ingest"


# ─── LibrarySkillError contract ───────────────────────────────────────


def test_library_skill_error_carries_code() -> None:
    exc = LibrarySkillError("boom", code="FOO")
    assert exc.code == "FOO"
    assert str(exc) == "boom"


def test_library_skill_error_is_a_wheelhouse_error() -> None:
    """``except wheelhouse.WheelhouseError:`` must catch it — parity
    with the existing error hierarchy."""
    from wheelhouse.errors import WheelhouseError

    exc = LibrarySkillError("oops", code="X")
    assert isinstance(exc, WheelhouseError)


def test_ingest_pipeline_raises_not_implemented(
    sandbox_mock: MagicMock,
) -> None:
    """The extension seam that 13-8 / 13-9 replace."""
    with pytest.raises(LibrarySkillError) as excinfo:
        ingest_mod._ingest_pipeline(
            sandbox_mock,
            {"source_type": "text", "source_ref": "foo"},
        )
    assert excinfo.value.code == LIBRARY_INGEST_NOT_IMPLEMENTED


# ─── AC-12: No real git in this test file ─────────────────────────────


def test_no_subprocess_invocation_during_scaffolding(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Monkeypatch ``subprocess.run`` to fail loudly — if any code path
    in the scaffold tried to shell out to git, this test would crash."""
    import subprocess

    def _boom(*args: object, **kwargs: object) -> None:
        raise RuntimeError(
            "AC-12 violation: scaffold path must never call subprocess.run"
        )

    monkeypatch.setattr(subprocess, "run", _boom)

    # Every top-level branch of run_library_ingest, once.
    run_library_ingest(None, {}, library_status="disabled", invocation_id="a")
    run_library_ingest(
        sandbox_mock, {}, library_status="enabled", invocation_id="b"
    )
    run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="read-only",
        invocation_id="c",
    )
    run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="enabled",
        invocation_id="d",
    )
