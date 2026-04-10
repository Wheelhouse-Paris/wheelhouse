"""Unit tests for the Library piggyback fields on SkillResult (Story 13-27).

Verifies the proto-level contract for FR35 / Epic 13 Cross-Codebase Contract §1:
the existing SkillResult message carries Library usage telemetry via three new
optional fields (library_tokens, library_page_count, library_last_ingest_at).

These tests live at the proto layer because the field-population logic itself
is owned by the still-to-be-built Library skills (13-7..13-18). What we
validate here is the wire contract: the fields exist, they round-trip, and
adding them did not break decoding of legacy-shaped SkillResult bytes.
"""

from __future__ import annotations

from wheelhouse.types import SkillResult


def test_skill_result_library_fields_round_trip() -> None:
    """AC-4: All three Library piggyback fields survive serialize+parse."""
    original = SkillResult(
        invocation_id="inv-123",
        skill_name="library_ingest",
        success=True,
        output="Ingested 12 pages from client-brief.pdf",
        timestamp_ms=1_712_750_000_000,
        library_tokens=4096,
        library_page_count=12,
        library_last_ingest_at="2026-04-10T14:30:00Z",
    )

    wire_bytes = bytes(original)
    parsed = SkillResult().parse(wire_bytes)

    # New Library piggyback fields
    assert parsed.library_tokens == 4096
    assert parsed.library_page_count == 12
    assert parsed.library_last_ingest_at == "2026-04-10T14:30:00Z"

    # Existing fields untouched
    assert parsed.invocation_id == "inv-123"
    assert parsed.skill_name == "library_ingest"
    assert parsed.success is True
    assert parsed.output == "Ingested 12 pages from client-brief.pdf"
    assert parsed.timestamp_ms == 1_712_750_000_000


def test_skill_result_legacy_bytes_decode_with_default_library_fields() -> None:
    """AC-5: A SkillResult that predates this story decodes cleanly.

    Backward-compatibility check: a SkillResult constructed with only the
    legacy fields (1..7) — i.e. the exact shape that older agents and the
    broker have been emitting since Epic 5 — must still decode under the new
    schema, with the new optional Library fields taking their proto3 defaults
    (None for proto3 explicit-presence optional fields in betterproto).
    """
    legacy = SkillResult(
        invocation_id="inv-legacy",
        skill_name="echo",
        success=True,
        output="hello",
        error_message="",
        error_code="",
        timestamp_ms=1_700_000_000_000,
    )
    legacy_bytes = bytes(legacy)

    parsed = SkillResult().parse(legacy_bytes)

    # Legacy fields preserved
    assert parsed.invocation_id == "inv-legacy"
    assert parsed.skill_name == "echo"
    assert parsed.success is True
    assert parsed.output == "hello"
    assert parsed.timestamp_ms == 1_700_000_000_000

    # New optional fields are unset (None) when never assigned — proto3
    # explicit-presence optional semantics. This is the wire-compatibility
    # guarantee: a missing optional uint32 / string is distinguishable from
    # an explicitly-set zero value.
    assert parsed.library_tokens is None
    assert parsed.library_page_count is None
    assert parsed.library_last_ingest_at is None


def test_skill_result_library_fields_default_to_none_when_not_set() -> None:
    """A freshly-constructed SkillResult has the Library fields unset."""
    result = SkillResult(invocation_id="x", skill_name="y", success=True)
    assert result.library_tokens is None
    assert result.library_page_count is None
    assert result.library_last_ingest_at is None
