"""Core types for the librarian decision policy (ADR-046, ADR-047)."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable

# V1 reason codes per ADR-047.  This list is a versioned API surface —
# existing codes are never removed or renamed once V1 ships.
REASON_CODES_V1: frozenset[str] = frozenset(
    {
        "durable_fact_written",
        "update_existing_page",
        "dedup_merged",
        "no_durable_fact_detected",
        "pii_not_durable",
        "contains_secret_pattern",
        "transient_context",
        "below_locale_confidence",
        "decision_timeout",
        "decision_error",
    }
)

# Type alias for the LLM injection point.
# (system_prompt, user_content) -> response_text
LlmFn = Callable[[str, str], str]


@dataclass(frozen=True)
class ConversationMessage:
    """A single message in a conversation segment (ADR-042)."""

    role: str  # "user" or "assistant"
    content: str
    timestamp_ms: int = 0


@dataclass(frozen=True)
class LibraryState:
    """Snapshot of the Library provided to ``decide()`` for context.

    ``index_content`` is the full text of ``index.md`` (empty string if absent).
    ``existing_pages`` maps page paths to their content, enabling update and
    merge detection.
    """

    index_content: str = ""
    existing_pages: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class DecisionResult:
    """Outcome of a single librarian decision (ADR-047).

    All golden-corpus assertions compare every field of this dataclass.
    """

    reason: str
    committed: bool
    page_path: str | None = None
    content: str | None = None
    schema_version: int = 1

    def __post_init__(self) -> None:
        if self.reason not in REASON_CODES_V1:
            raise ValueError(
                f"Unknown reason code {self.reason!r}; "
                f"valid V1 codes: {sorted(REASON_CODES_V1)}"
            )
