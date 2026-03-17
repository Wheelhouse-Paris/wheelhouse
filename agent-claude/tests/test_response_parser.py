"""Tests for response_parser module (Story 10.4, ADR-022).

Tests cover:
  - Valid single-item, multi-item, and empty array parsing
  - Malformed JSON rejection
  - Missing/wrong-type key rejection
  - Non-array JSON rejection
  - Markdown code fence stripping
  - Batch output instruction formatting
"""

from __future__ import annotations

import json
import logging

import pytest

from agent_claude.response_parser import (
    format_batch_instruction,
    parse_batch_response,
)


# ---------------------------------------------------------------------------
# Valid parsing tests (AC-1, AC-2, AC-3)
# ---------------------------------------------------------------------------


class TestParseValidResponses:
    """Tests for valid batch response parsing."""

    def test_single_item_array(self) -> None:
        """AC-3: Single-item array returns list of 1."""
        raw = json.dumps([
            {"stream": "main", "type": "TextMessage", "content": "Hello"}
        ])
        result = parse_batch_response(raw)
        assert result is not None
        assert len(result) == 1
        assert result[0]["stream"] == "main"
        assert result[0]["content"] == "Hello"

    def test_multi_item_array(self) -> None:
        """AC-1: Multi-item array returns list of N."""
        items = [
            {"stream": "main", "type": "TextMessage", "content": "Hi main"},
            {"stream": "logs", "type": "TextMessage", "content": "Logged"},
            {"stream": "alerts", "type": "TextMessage", "content": "Alert!"},
        ]
        raw = json.dumps(items)
        result = parse_batch_response(raw)
        assert result is not None
        assert len(result) == 3
        assert result[0]["stream"] == "main"
        assert result[1]["stream"] == "logs"
        assert result[2]["stream"] == "alerts"

    def test_empty_array(self) -> None:
        """AC-2: Empty array returns empty list (no-op)."""
        result = parse_batch_response("[]")
        assert result is not None
        assert result == []

    def test_extra_keys_allowed(self) -> None:
        """Extra keys in items are allowed (forward compat)."""
        raw = json.dumps([
            {"stream": "main", "type": "TextMessage", "content": "Hi", "meta": "extra"}
        ])
        result = parse_batch_response(raw)
        assert result is not None
        assert len(result) == 1


# ---------------------------------------------------------------------------
# Malformed response tests (AC-4)
# ---------------------------------------------------------------------------


class TestParseMalformedResponses:
    """Tests for malformed batch response rejection."""

    def test_malformed_json(self) -> None:
        """AC-4: Malformed JSON returns None."""
        result = parse_batch_response("not json at all {{{")
        assert result is None

    def test_missing_stream_key(self) -> None:
        """AC-4: Missing 'stream' key returns None."""
        raw = json.dumps([
            {"type": "TextMessage", "content": "Hello"}
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_missing_type_key(self) -> None:
        """AC-4: Missing 'type' key returns None."""
        raw = json.dumps([
            {"stream": "main", "content": "Hello"}
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_missing_content_key(self) -> None:
        """AC-4: Missing 'content' key returns None."""
        raw = json.dumps([
            {"stream": "main", "type": "TextMessage"}
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_wrong_type_field(self) -> None:
        """AC-4: Unsupported type field returns None."""
        raw = json.dumps([
            {"stream": "main", "type": "ImageMessage", "content": "Hello"}
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_non_array_json_dict(self) -> None:
        """AC-4: JSON dict (not array) returns None."""
        raw = json.dumps({"stream": "main", "type": "TextMessage", "content": "Hi"})
        result = parse_batch_response(raw)
        assert result is None

    def test_non_array_json_string(self) -> None:
        """AC-4: JSON string returns None."""
        raw = json.dumps("just a string")
        result = parse_batch_response(raw)
        assert result is None

    def test_empty_stream_name(self) -> None:
        """AC-4: Empty stream name returns None."""
        raw = json.dumps([
            {"stream": "", "type": "TextMessage", "content": "Hello"}
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_empty_content(self) -> None:
        """AC-4: Empty content returns None."""
        raw = json.dumps([
            {"stream": "main", "type": "TextMessage", "content": ""}
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_non_string_stream(self) -> None:
        """AC-4: Non-string stream returns None."""
        raw = json.dumps([
            {"stream": 42, "type": "TextMessage", "content": "Hello"}
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_non_dict_item(self) -> None:
        """AC-4: Non-dict item in array returns None."""
        raw = json.dumps(["just a string"])
        result = parse_batch_response(raw)
        assert result is None

    def test_partial_valid_batch_rejected(self) -> None:
        """AC-4: If one item is invalid, entire batch rejected (no partial publish)."""
        raw = json.dumps([
            {"stream": "main", "type": "TextMessage", "content": "Good"},
            {"stream": "logs", "type": "BadType", "content": "Bad"},
        ])
        result = parse_batch_response(raw)
        assert result is None

    def test_malformed_logs_error(self, caplog: pytest.LogCaptureFixture) -> None:
        """AC-4: Malformed response logs error message."""
        with caplog.at_level(logging.ERROR, logger="agent_claude"):
            parse_batch_response("not json")
        assert any("not valid JSON" in r.message for r in caplog.records)


# ---------------------------------------------------------------------------
# Markdown code fence stripping
# ---------------------------------------------------------------------------


class TestCodeFenceStripping:
    """Tests for stripping markdown code fences from LLM output."""

    def test_json_code_fence(self) -> None:
        """JSON wrapped in ```json ... ``` is parsed correctly."""
        inner = json.dumps([{"stream": "main", "type": "TextMessage", "content": "Hi"}])
        raw = f"```json\n{inner}\n```"
        result = parse_batch_response(raw)
        assert result is not None
        assert len(result) == 1
        assert result[0]["content"] == "Hi"

    def test_plain_code_fence(self) -> None:
        """JSON wrapped in ``` ... ``` is parsed correctly."""
        inner = json.dumps([{"stream": "main", "type": "TextMessage", "content": "Hi"}])
        raw = f"```\n{inner}\n```"
        result = parse_batch_response(raw)
        assert result is not None
        assert len(result) == 1

    def test_no_fence(self) -> None:
        """Raw JSON without fences works normally."""
        raw = json.dumps([{"stream": "main", "type": "TextMessage", "content": "Hi"}])
        result = parse_batch_response(raw)
        assert result is not None
        assert len(result) == 1


# ---------------------------------------------------------------------------
# Batch output instruction formatting (AC-5)
# ---------------------------------------------------------------------------


class TestBatchInstruction:
    """Tests for the batch output instruction formatting."""

    def test_includes_stream_names(self) -> None:
        """AC-5: Instruction includes all stream names."""
        instruction = format_batch_instruction(["main", "logs", "alerts"])
        assert "main" in instruction
        assert "logs" in instruction
        assert "alerts" in instruction

    def test_includes_json_format(self) -> None:
        """AC-5: Instruction includes JSON format description."""
        instruction = format_batch_instruction(["main"])
        assert "JSON array" in instruction
        assert "TextMessage" in instruction

    def test_includes_empty_array_instruction(self) -> None:
        """AC-5: Instruction mentions empty array for no-op."""
        instruction = format_batch_instruction(["main"])
        assert "[]" in instruction

    def test_empty_streams(self) -> None:
        """Instruction handles empty stream list gracefully."""
        instruction = format_batch_instruction([])
        assert "(none)" in instruction

    def test_output_format_header(self) -> None:
        """Instruction starts with markdown header."""
        instruction = format_batch_instruction(["main"])
        assert instruction.startswith("## Output Format")
