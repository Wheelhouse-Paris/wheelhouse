"""Golden corpus parity tests for the librarian decision policy.

Each JSON sample in ``tests/golden/librarian/`` encodes a conversation
segment, a mock LLM response, and the expected ``DecisionResult``.  The
tests call ``decide()`` with a deterministic ``llm_fn`` that returns the
canned response and verify byte-level parity of the output.

No real LLM calls are made.  No I/O, ZMQ, or external dependencies.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from wheelhouse.skills.librarian import ConversationMessage, LibraryState, decide
from wheelhouse.skills.librarian.types import DecisionResult, REASON_CODES_V1

GOLDEN_DIR = Path(__file__).parent / "golden" / "librarian"


def _load_samples() -> list[tuple[str, dict]]:
    """Discover and load all golden corpus JSON files."""
    samples: list[tuple[str, dict]] = []
    for path in sorted(GOLDEN_DIR.glob("*.json")):
        with open(path) as f:
            data = json.load(f)
        samples.append((path.stem, data))
    return samples


SAMPLES = _load_samples()
SAMPLE_IDS = [name for name, _ in SAMPLES]


@pytest.mark.parametrize("sample", [s for _, s in SAMPLES], ids=SAMPLE_IDS)
def test_golden_corpus_parity(sample: dict) -> None:
    """decide() output matches expected golden output for each sample."""
    inp = sample["input"]
    expected = sample["expected"]

    segment = [
        ConversationMessage(
            role=m["role"],
            content=m["content"],
            timestamp_ms=m.get("timestamp_ms", 0),
        )
        for m in inp["segment"]
    ]

    library_state = LibraryState(
        index_content=inp["library_state"].get("index_content", ""),
        existing_pages=inp["library_state"].get("existing_pages", {}),
    )

    mock_response = inp["mock_llm_response"]

    def mock_llm_fn(system_prompt: str, user_content: str) -> str:
        return mock_response

    result = decide(
        segment=segment,
        library_state=library_state,
        locale=inp["locale"],
        llm_fn=mock_llm_fn,
    )

    assert result.reason == expected["reason"], (
        f"reason mismatch: got {result.reason!r}, expected {expected['reason']!r}"
    )
    assert result.committed == expected["committed"], (
        f"committed mismatch: got {result.committed!r}, expected {expected['committed']!r}"
    )
    assert result.page_path == expected["page_path"], (
        f"page_path mismatch:\n  got:      {result.page_path!r}\n  expected: {expected['page_path']!r}"
    )
    assert result.content == expected["content"], (
        f"content mismatch:\n  got:      {result.content!r}\n  expected: {expected['content']!r}"
    )
    assert result.schema_version == expected["schema_version"], (
        f"schema_version mismatch: got {result.schema_version}, expected {expected['schema_version']}"
    )


def test_deterministic_output() -> None:
    """Running decide() twice with identical inputs produces identical results."""
    if not SAMPLES:
        pytest.skip("No golden samples found")

    _, sample = SAMPLES[0]
    inp = sample["input"]

    segment = [
        ConversationMessage(
            role=m["role"],
            content=m["content"],
            timestamp_ms=m.get("timestamp_ms", 0),
        )
        for m in inp["segment"]
    ]
    library_state = LibraryState(
        index_content=inp["library_state"].get("index_content", ""),
        existing_pages=inp["library_state"].get("existing_pages", {}),
    )

    def mock_llm_fn(system_prompt: str, user_content: str) -> str:
        return inp["mock_llm_response"]

    result1 = decide(segment=segment, library_state=library_state, locale=inp["locale"], llm_fn=mock_llm_fn)
    result2 = decide(segment=segment, library_state=library_state, locale=inp["locale"], llm_fn=mock_llm_fn)

    assert result1 == result2, f"Non-deterministic output:\n  run1: {result1}\n  run2: {result2}"


def test_reason_code_coverage() -> None:
    """Golden corpus covers at least 5 of the 10 V1 reason codes."""
    covered_reasons = {sample["expected"]["reason"] for _, sample in SAMPLES}
    assert covered_reasons.issubset(REASON_CODES_V1), (
        f"Unknown reason codes in samples: {covered_reasons - REASON_CODES_V1}"
    )
    assert len(covered_reasons) >= 5, (
        f"Only {len(covered_reasons)} reason codes covered ({covered_reasons}); need at least 5"
    )


def test_samples_have_required_fields() -> None:
    """Each sample contains all required input and expected fields."""
    for name, sample in SAMPLES:
        assert "input" in sample, f"{name}: missing 'input'"
        assert "expected" in sample, f"{name}: missing 'expected'"

        inp = sample["input"]
        assert "segment" in inp, f"{name}: missing input.segment"
        assert "locale" in inp, f"{name}: missing input.locale"
        assert "library_state" in inp, f"{name}: missing input.library_state"
        assert "mock_llm_response" in inp, f"{name}: missing input.mock_llm_response"

        exp = sample["expected"]
        assert "reason" in exp, f"{name}: missing expected.reason"
        assert "committed" in exp, f"{name}: missing expected.committed"
        assert "schema_version" in exp, f"{name}: missing expected.schema_version"


def test_decide_handles_llm_error() -> None:
    """decide() returns decision_error when llm_fn raises an exception."""
    segment = [ConversationMessage(role="user", content="test")]
    library_state = LibraryState()

    def failing_llm_fn(system_prompt: str, user_content: str) -> str:
        raise RuntimeError("LLM unavailable")

    result = decide(segment=segment, library_state=library_state, locale="en", llm_fn=failing_llm_fn)
    assert result.reason == "decision_error"
    assert result.committed is False
    assert result.page_path is None
    assert result.content is None


def test_decide_handles_unknown_reason_code() -> None:
    """decide() returns decision_error when LLM returns an unknown reason code."""
    segment = [ConversationMessage(role="user", content="test")]
    library_state = LibraryState()

    def unknown_reason_llm_fn(system_prompt: str, user_content: str) -> str:
        return '{"reason": "made_up_code", "committed": false, "page_path": null, "content": null}'

    result = decide(segment=segment, library_state=library_state, locale="en", llm_fn=unknown_reason_llm_fn)
    assert result.reason == "decision_error"
    assert result.committed is False


def test_decide_handles_malformed_json() -> None:
    """decide() returns decision_error when llm_fn returns invalid JSON."""
    segment = [ConversationMessage(role="user", content="test")]
    library_state = LibraryState()

    def bad_json_llm_fn(system_prompt: str, user_content: str) -> str:
        return "This is not JSON at all"

    result = decide(segment=segment, library_state=library_state, locale="en", llm_fn=bad_json_llm_fn)
    assert result.reason == "decision_error"
    assert result.committed is False
