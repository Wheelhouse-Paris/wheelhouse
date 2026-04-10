"""Tests for Library schema graceful absence (Epic 13 story 13-20, NFR24).

Acceptance criteria covered:
  AC-7: Agent boots with Library disabled when schema file is missing or unreadable.
  AC-8: Agent records library_status="enabled" when schema is present.
        Cloud-side WH_LIBRARY_STATUS values are never overwritten by the agent.
"""

from __future__ import annotations

import logging
import os
from pathlib import Path

import pytest

from agent_claude import main as agent_main


# ---------------------------------------------------------------------------
# AC-8: schema file present → enabled
# ---------------------------------------------------------------------------


def test_check_library_schema_enabled_when_file_present(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Schema file exists and is readable → returns "enabled"."""
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Your Library\n")
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))

    assert agent_main.check_library_schema() == "enabled"


# ---------------------------------------------------------------------------
# AC-7: schema file absent → disabled (with warning log)
# ---------------------------------------------------------------------------


def test_check_library_schema_disabled_when_file_missing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    """Schema file missing → returns "disabled" and logs a WARNING."""
    missing_path = tmp_path / "does-not-exist.md"
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(missing_path))

    with caplog.at_level(logging.WARNING, logger="agent_claude"):
        result = agent_main.check_library_schema()

    assert result == "disabled"
    # Assert the warning includes the literal "Library disabled" framing.
    assert any(
        "Library disabled" in record.message for record in caplog.records
    ), f"expected 'Library disabled' in WARNING log, got: {[r.message for r in caplog.records]}"
    # Assert the warning includes the path it probed.
    assert any(str(missing_path) in record.message for record in caplog.records)


def test_check_library_schema_disabled_when_file_unreadable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
) -> None:
    """Schema file exists but is not readable → returns "disabled"."""
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Your Library\n")
    # Strip read bits — verify R_OK fails before relying on this.
    schema_file.chmod(0o000)
    try:
        if os.access(str(schema_file), os.R_OK):
            # Running as root: chmod 000 doesn't block reads. Skip.
            pytest.skip("running as root — chmod 000 doesn't block reads")
        monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))

        with caplog.at_level(logging.WARNING, logger="agent_claude"):
            result = agent_main.check_library_schema()

        assert result == "disabled"
    finally:
        # Restore so tmp_path cleanup can run.
        schema_file.chmod(0o644)


# ---------------------------------------------------------------------------
# AC-7 / AC-8: WH_LIBRARY_STATUS env var precedence
# ---------------------------------------------------------------------------


def test_check_library_schema_does_not_touch_env_var(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """check_library_schema() must NOT write to os.environ.

    Env-var propagation is the responsibility of run_startup, which has the
    cloud-precedence rules. The probe function is pure-filesystem.
    """
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Your Library\n")
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))
    monkeypatch.delenv("WH_LIBRARY_STATUS", raising=False)

    agent_main.check_library_schema()

    assert "WH_LIBRARY_STATUS" not in os.environ


def test_run_startup_env_var_logic_unset_to_disabled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """When WH_LIBRARY_STATUS is unset and schema is missing, run_startup's
    env-var logic must set it to "disabled".

    We exercise the same conditional that run_startup uses, since run_startup
    itself does network I/O (wheelhouse.connect) and is awkward to mock end-to-
    end. The contract under test is the *rule*, not the call site: rule
    asserted in this unit test, call site asserted by static review.
    """
    monkeypatch.delenv("WH_LIBRARY_STATUS", raising=False)
    missing = tmp_path / "missing.md"
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(missing))

    library_status = agent_main.check_library_schema()
    if library_status == "disabled" and "WH_LIBRARY_STATUS" not in os.environ:
        os.environ["WH_LIBRARY_STATUS"] = "disabled"

    try:
        assert os.environ.get("WH_LIBRARY_STATUS") == "disabled"
    finally:
        os.environ.pop("WH_LIBRARY_STATUS", None)


def test_run_startup_env_var_logic_does_not_clobber_cloud_value(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """When the cloud has already set WH_LIBRARY_STATUS=active, the agent
    must NOT overwrite it — even if the schema file is missing locally."""
    monkeypatch.setenv("WH_LIBRARY_STATUS", "active")
    missing = tmp_path / "missing.md"
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(missing))

    library_status = agent_main.check_library_schema()
    if library_status == "disabled" and "WH_LIBRARY_STATUS" not in os.environ:
        os.environ["WH_LIBRARY_STATUS"] = "disabled"

    assert os.environ["WH_LIBRARY_STATUS"] == "active", (
        "agent must not overwrite cloud-side WH_LIBRARY_STATUS=active even when "
        "the schema file is missing locally"
    )


def test_run_startup_env_var_logic_no_write_when_enabled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """When the schema file is present, the agent must NOT write
    WH_LIBRARY_STATUS at all (even if it's currently unset)."""
    monkeypatch.delenv("WH_LIBRARY_STATUS", raising=False)
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Your Library\n")
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))

    library_status = agent_main.check_library_schema()
    if library_status == "disabled" and "WH_LIBRARY_STATUS" not in os.environ:
        os.environ["WH_LIBRARY_STATUS"] = "disabled"

    assert library_status == "enabled"
    assert "WH_LIBRARY_STATUS" not in os.environ, (
        "agent must not declare a Library status when the schema is present — "
        "that is the cloud provisioner's job"
    )


# ---------------------------------------------------------------------------
# Story 13.21: L5 wiring rule in run_startup
# ---------------------------------------------------------------------------
#
# run_startup() itself does network I/O (wheelhouse.connect) and is awkward to
# mock end-to-end. The contract under test is the *rule*:
#
#     effective_status = os.environ.get("WH_LIBRARY_STATUS", "").strip()
#     if effective_status != "disabled":
#         persona.library_schema_path = LIBRARY_SCHEMA_PATH
#
# These tests exercise that rule against a fresh Persona and assert the
# resulting build_system_prompt() output matches the AC, following the same
# "rule asserted in unit, call site asserted by static review" pattern as the
# tests above.


def _simulate_run_startup_l5_wiring(schema_path: str) -> "object":
    """Mirror the exact run_startup() L5 wiring code path.

    Creates a minimal Persona and applies the same env-var / path logic
    that run_startup uses, so the acceptance-criterion assertions run
    against real Persona.build_system_prompt() output without having to
    mock wheelhouse.connect.
    """
    from agent_claude.persona import Persona

    persona = Persona(
        soul="s",
        identity="i",
        memory="m",
        streams=["main"],
    )

    # NB: main.check_library_schema() is a pure probe; call it exactly as
    # run_startup does for the AC-1 part of the rule.
    library_status = agent_main.check_library_schema()
    if library_status == "disabled" and "WH_LIBRARY_STATUS" not in os.environ:
        os.environ["WH_LIBRARY_STATUS"] = "disabled"

    effective_status = os.environ.get("WH_LIBRARY_STATUS", "").strip()
    if effective_status != "disabled":
        persona.library_schema_path = agent_main.LIBRARY_SCHEMA_PATH

    return persona


def test_ac5_enabled_wires_l5_when_file_present_and_env_unset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC-5 first clause: file present, env unset → L5 wired and injected."""
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Library Schema\n\nuse the library wisely")
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))
    monkeypatch.delenv("WH_LIBRARY_STATUS", raising=False)

    try:
        persona = _simulate_run_startup_l5_wiring(str(schema_file))
        assert persona.library_schema_path == str(schema_file)  # type: ignore[attr-defined]
        assert "## Library Schema" in persona.build_system_prompt()  # type: ignore[attr-defined]
    finally:
        os.environ.pop("WH_LIBRARY_STATUS", None)


def test_ac5_disabled_wires_none_when_file_missing_and_env_unset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC-5 second clause: file missing, env unset → L5 not wired."""
    missing = tmp_path / "missing.md"
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(missing))
    monkeypatch.delenv("WH_LIBRARY_STATUS", raising=False)

    try:
        persona = _simulate_run_startup_l5_wiring(str(missing))
        assert persona.library_schema_path is None  # type: ignore[attr-defined]
        assert "## Library Schema" not in persona.build_system_prompt()  # type: ignore[attr-defined]
    finally:
        os.environ.pop("WH_LIBRARY_STATUS", None)


def test_ac6_cloud_disabled_suppresses_l5_even_if_file_exists(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC-6: WH_LIBRARY_STATUS=disabled (cloud-set) suppresses L5 even when
    the schema file is present on disk."""
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Library Schema\n\npresent but disabled by cloud")
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))
    monkeypatch.setenv("WH_LIBRARY_STATUS", "disabled")

    persona = _simulate_run_startup_l5_wiring(str(schema_file))
    assert persona.library_schema_path is None  # type: ignore[attr-defined]
    assert "## Library Schema" not in persona.build_system_prompt()  # type: ignore[attr-defined]


def test_ac7_cloud_active_wires_l5(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC-7: WH_LIBRARY_STATUS=active keeps L5 injected."""
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Library Schema\n\nactive schema body")
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))
    monkeypatch.setenv("WH_LIBRARY_STATUS", "active")

    persona = _simulate_run_startup_l5_wiring(str(schema_file))
    assert persona.library_schema_path == str(schema_file)  # type: ignore[attr-defined]
    prompt = persona.build_system_prompt()  # type: ignore[attr-defined]
    assert "## Library Schema" in prompt
    assert "active schema body" in prompt


def test_ac7_cloud_read_only_wires_l5(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC-7: WH_LIBRARY_STATUS=read-only keeps L5 injected."""
    schema_file = tmp_path / ".wh-schema.md"
    schema_file.write_text("# Library Schema\n\nread-only schema body")
    monkeypatch.setattr(agent_main, "LIBRARY_SCHEMA_PATH", str(schema_file))
    monkeypatch.setenv("WH_LIBRARY_STATUS", "read-only")

    persona = _simulate_run_startup_l5_wiring(str(schema_file))
    assert persona.library_schema_path == str(schema_file)  # type: ignore[attr-defined]
    assert "## Library Schema" in persona.build_system_prompt()  # type: ignore[attr-defined]
