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
    LIBRARY_INGEST_NO_SUMMARIZER,
    LIBRARY_INGEST_NOT_IMPLEMENTED,
    LIBRARY_READ_ONLY,
    REQUIRED_PARAMS,
    SKILL_NAME,
    SKILL_REGISTRY,
    run_library_ingest,
    set_summarizer,
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


# ─── AC-9: No-summarizer default (updated by 13-8) ────────────────────
#
# 13-7 shipped a NOT_IMPLEMENTED sentinel; 13-8 replaced the pipeline
# body with the real text/markdown path and the new default when no
# summarizer is wired is LIBRARY_INGEST_NO_SUMMARIZER. Validation
# passes but the pipeline refuses BEFORE opening a transaction.


def test_run_library_ingest_happy_path_without_summarizer(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
) -> None:
    set_summarizer(None)  # explicit default
    result = run_library_ingest(
        sandbox_mock,
        valid_params,
        library_status="enabled",
        invocation_id="inv-009",
    )
    assert result.success is False
    assert result.error_code == LIBRARY_INGEST_NO_SUMMARIZER
    # Validation passed but no transaction was opened — the
    # no-summarizer path never touches the sandbox write surface.
    sandbox_mock.begin.assert_not_called()
    sandbox_mock.transaction.assert_not_called()


def test_run_library_ingest_echoes_invocation_id_and_skill_name(
    sandbox_mock: MagicMock,
    valid_params: dict[str, str],
) -> None:
    set_summarizer(None)
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


def test_ingest_pipeline_unsupported_type_after_13_9(
    sandbox_mock: MagicMock,
) -> None:
    """13-7 raised NOT_IMPLEMENTED unconditionally. 13-8 shipped the
    text/markdown body and raised UNSUPPORTED_TYPE for pdf/url. 13-9
    replaced the pdf branch with real extraction, so only ``url``
    remains unimplemented — this test pins that refusal without losing
    the 13-7/13-8 coverage. NOT_IMPLEMENTED stays in the catalogue as
    the historical sentinel.
    """
    assert LIBRARY_INGEST_NOT_IMPLEMENTED  # constant still exported
    with pytest.raises(LibrarySkillError) as excinfo:
        ingest_mod._ingest_pipeline(
            sandbox_mock,
            {"source_type": "url", "source_ref": "https://example.com/x"},
        )
    assert excinfo.value.code == ingest_mod.LIBRARY_INGEST_UNSUPPORTED_TYPE


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
    # 13-8: make sandbox.read return a real string so the source
    # resolver doesn't blow up on a MagicMock comparison, and ensure
    # no summarizer is wired so the pipeline short-circuits before
    # opening a transaction.
    sandbox_mock.read.return_value = "# brief\n"
    set_summarizer(None)

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
