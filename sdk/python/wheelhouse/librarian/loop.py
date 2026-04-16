"""LibrarianLoop — event consumer and decision dispatch.

Implements the 10-step decision loop skeleton from ADR-046.
Steps 1-5 and 8-10 are wired here; step 6-7 (LLM call + parse)
are delegated to decide() which is implemented in story 14-1-3.

This story (14-1-2) provides the skeleton: process_event() logs the
event and returns a placeholder DecisionResult. The full decision
policy is plugged in by story 14-1-3.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any, Callable

from wheelhouse.librarian.proto import LibraryWriteEvent
from wheelhouse.librarian.types import DecisionResult

logger = logging.getLogger("wheelhouse.librarian")


class LibrarianLoop:
    """Main event processing loop for the librarian runtime.

    Processes LibraryWriteEvent messages sequentially (single-threaded
    per ADR-046) to prevent git commit conflicts.

    Args:
        library_path: Absolute path to the Library root (RW-mounted volume).
        library_id: Library identifier for decision log attribution.
        locales: List of supported locale codes (e.g. ['en', 'fr']).
        llm_fn: Callable (system_prompt, user_content) -> response string.
                None in this story; plugged in by 14-1-3.
    """

    def __init__(
        self,
        library_path: str | Path,
        library_id: str,
        locales: list[str] | None = None,
        llm_fn: Callable[[str, str], str] | None = None,
    ) -> None:
        self.library_path = Path(library_path)
        self.library_id = library_id
        self.locales = locales or ["en", "fr"]
        self.llm_fn = llm_fn

    def process_event(self, event: LibraryWriteEvent) -> DecisionResult:
        """Process a single LibraryWriteEvent.

        This is the dispatch target called by the __main__ message handler.
        In this story (14-1-2), it logs the event and returns a placeholder.
        Story 14-1-3 plugs in the full decision policy.

        Args:
            event: Deserialized LibraryWriteEvent from the broker.

        Returns:
            DecisionResult with the processing outcome.
        """
        logger.info(
            "Processing event: event_id=%s source_agent_id=%s library_id=%s locale=%s",
            event.event_id,
            event.source_agent_id,
            event.library_id,
            event.locale,
        )

        # Placeholder: full decision logic comes in story 14-1-3
        return DecisionResult(
            reason="pending",
            committed=False,
            event_id=event.event_id,
            locale=event.locale or "en",
        )
