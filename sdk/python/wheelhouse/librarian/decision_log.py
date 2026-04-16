"""Structured JSON decision logging for the librarian (ADR-047).

Every process_event() call emits a structured log entry to stdout
as a single-line JSON object. On framework runtime, ``wh logs``
captures these. On cloud, the same format goes to CloudWatch.

Story: 14-1-5
FR: FR8, FR9, FR28, FR29
NFR: NFR20 (200-char snippet PII boundary), NFR21, NFR24
"""

from __future__ import annotations

import json
import logging
import time
from dataclasses import asdict, dataclass, field
from typing import Any

logger = logging.getLogger("wheelhouse.librarian.decision")


@dataclass
class StepSpan:
    """A single timing span within a decision trace."""

    name: str
    duration_ms: int


@dataclass
class DecisionLogEntry:
    """Structured decision log entry per ADR-047.

    All fields are populated by the librarian loop after each decision.
    ``snippet`` is capped at 200 chars (NFR20 PII boundary).
    """

    schema_version: int
    event_id: str
    library_id: str
    source_agent_id: str
    conversation_id: str
    reason: str
    committed: bool
    commit_hash: str | None
    locale: str
    tokens_consumed: int
    snippet: str
    timestamp: str
    duration_ms: int
    step_spans: list[dict[str, Any]] = field(default_factory=list)


def build_snippet(segment_texts: list[str], max_len: int = 200) -> str:
    """Build a PII-safe snippet from conversation segment texts.

    Concatenates all segment texts with " | " separator and truncates
    to ``max_len`` characters (NFR20 PII boundary, 30-day retention).

    Args:
        segment_texts: List of message content strings.
        max_len: Maximum snippet length (default 200).

    Returns:
        Truncated snippet string.
    """
    combined = " | ".join(segment_texts)
    if len(combined) > max_len:
        return combined[:max_len]
    return combined


def emit_decision_log(entry: DecisionLogEntry) -> None:
    """Emit a structured decision log entry as a single-line JSON to stdout.

    Uses the ``wheelhouse.librarian.decision`` logger at INFO level.
    The JSON is emitted as the log message content, designed to be
    captured by ``wh logs`` (framework) or CloudWatch (cloud).
    """
    log_dict = asdict(entry)
    # Single-line JSON — no pretty-printing for log aggregation.
    json_str = json.dumps(log_dict, separators=(",", ":"), ensure_ascii=False)
    logger.info(json_str)


class SpanTimer:
    """Context manager for timing step spans.

    Usage::

        timer = SpanTimer()
        with timer.span("llm_call"):
            result = llm_fn(prompt, content)
        spans = timer.spans  # [{"name": "llm_call", "duration_ms": 1234}]
    """

    def __init__(self) -> None:
        self.spans: list[dict[str, Any]] = []

    def span(self, name: str) -> _SpanContext:
        """Return a context manager that records a named span."""
        return _SpanContext(self, name)

    @property
    def total_ms(self) -> int:
        """Sum of all span durations."""
        return sum(s["duration_ms"] for s in self.spans)


class _SpanContext:
    """Context manager for a single timing span."""

    def __init__(self, timer: SpanTimer, name: str) -> None:
        self._timer = timer
        self._name = name
        self._start: float = 0.0

    def __enter__(self) -> _SpanContext:
        self._start = time.monotonic()
        return self

    def __exit__(self, *exc: object) -> None:
        elapsed_ms = int((time.monotonic() - self._start) * 1000)
        self._timer.spans.append({"name": self._name, "duration_ms": elapsed_ms})
