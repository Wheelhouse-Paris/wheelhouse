"""Data types for the librarian decision loop.

DecisionResult is the output of LibrarianLoop.process_event().
The full field set is populated by the decision policy (story 14-1-3);
this story provides the skeleton with all fields defined.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class DecisionResult:
    """Result of processing a single LibraryWriteEvent.

    Attributes:
        reason: One of the V1 reason codes (ADR-047 taxonomy).
        committed: True if a page was written and committed to git.
        event_id: The event_id from the source LibraryWriteEvent.
        locale: Detected locale used for this decision.
        page_path: Relative path of the written page (None if skipped).
        content: Page content that was written (None if skipped).
        tokens_consumed: LLM tokens used for this decision.
        schema_version: Always 1 in V1.
    """

    reason: str = "pending"
    committed: bool = False
    event_id: str = ""
    locale: str = ""
    page_path: str | None = None
    content: str | None = None
    tokens_consumed: int = 0
    schema_version: int = 1
