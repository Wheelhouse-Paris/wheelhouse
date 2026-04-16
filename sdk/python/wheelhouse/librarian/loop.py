"""LibrarianLoop — event consumer and decision dispatch.

Implements the decision loop from ADR-046. Converts proto types to
policy types, calls decide(), writes pages via LibrarySandbox, and
commits to git with structured metadata.

Stories:
  - 14-1-2: skeleton (process_event placeholder)
  - 14-1-3: full decision policy wiring
  - 14-1-4: idempotent event processing with LRU dedup cache
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Callable

from wheelhouse.librarian.dedup import DedupCache
from wheelhouse.librarian.proto import LibraryWriteEvent
from wheelhouse.librarian.types import DecisionResult
from wheelhouse.skills.librarian.decide import decide
from wheelhouse.skills.librarian.types import (
    ConversationMessage as PolicyConversationMessage,
    DecisionResult as PolicyDecisionResult,
    LibraryState,
)

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
        sandbox: LibrarySandbox instance for file operations and git commits.
                 None disables write operations (dry-run / test mode).
        dedup: DedupCache for idempotent event processing (story 14-1-4).
               None disables dedup checking (backward compat / test mode).
    """

    def __init__(
        self,
        library_path: str | Path,
        library_id: str,
        locales: list[str] | None = None,
        llm_fn: Callable[[str, str], str] | None = None,
        sandbox: object | None = None,
        dedup: DedupCache | None = None,
    ) -> None:
        self.library_path = Path(library_path)
        self.library_id = library_id
        self.locales = locales or ["en", "fr"]
        self.llm_fn = llm_fn
        self.sandbox = sandbox
        self.dedup = dedup

    def _build_library_state(self) -> LibraryState:
        """Read current Library state from the filesystem.

        Returns a LibraryState with the index.md content and a dict of
        existing page paths to their content. If the sandbox is not
        available or index.md does not exist, returns an empty state.
        """
        if self.sandbox is None:
            return LibraryState()

        index_content = ""
        try:
            if self.sandbox.exists("index.md"):
                index_content = self.sandbox.read("index.md")
        except Exception:
            logger.debug("Failed to read index.md, using empty index")

        existing_pages: dict[str, str] = {}
        try:
            all_files = self.sandbox.list(".")
            for fpath in all_files:
                if fpath.startswith("pages/") and fpath.endswith(".md"):
                    try:
                        existing_pages[fpath] = self.sandbox.read(fpath)
                    except Exception:
                        logger.debug("Failed to read page %s, skipping", fpath)
        except Exception:
            logger.debug("Failed to list library pages, using empty state")

        return LibraryState(
            index_content=index_content,
            existing_pages=existing_pages,
        )

    def _convert_segment(
        self, event: LibraryWriteEvent
    ) -> list[PolicyConversationMessage]:
        """Convert proto ConversationMessage list to policy types."""
        return [
            PolicyConversationMessage(
                role=msg.role,
                content=msg.content,
                timestamp_ms=msg.timestamp_ms,
            )
            for msg in event.segment
        ]

    def _write_and_commit(
        self,
        policy_result: PolicyDecisionResult,
        event: LibraryWriteEvent,
    ) -> str | None:
        """Write a page and commit to git. Returns commit hash or None."""
        if self.sandbox is None:
            logger.warning(
                "sandbox is None — cannot write page for event %s",
                event.event_id,
            )
            return None

        page_path = policy_result.page_path
        content = policy_result.content
        if not page_path or not content:
            logger.warning(
                "decide() returned committed=True but missing page_path/content "
                "for event %s",
                event.event_id,
            )
            return None

        reason = policy_result.reason
        is_update = reason in ("update_existing_page", "dedup_merged")

        # Begin transaction — include source_agent_id in summary for
        # multi-agent attribution (story 14-2-4, FR22, FR40).
        source = event.source_agent_id or "unknown"
        self.sandbox.begin_transaction(
            operation="librarian_decide",
            summary=f"{reason}: {page_path} (source: {source})",
        )

        try:
            # Write page
            self.sandbox.write(page_path, content)

            # Commit with structured metadata — sources carries the
            # originating agent so git log shows attribution (ADR-039).
            commit_kwargs: dict = {}
            if is_update:
                commit_kwargs["pages_updated"] = [page_path]
            else:
                commit_kwargs["pages_created"] = [page_path]
            commit_kwargs["sources"] = [source]
            self.sandbox.commit(**commit_kwargs)
        except Exception:
            logger.exception(
                "Failed to write/commit page %s for event %s",
                page_path,
                event.event_id,
            )
            try:
                self.sandbox.rollback()
            except Exception:
                logger.debug("Rollback after failed commit also failed")
            return None

        return None  # commit_hash extraction deferred to 14-1-5

    def process_event(self, event: LibraryWriteEvent) -> DecisionResult:
        """Process a single LibraryWriteEvent through the decision policy.

        Converts proto types to policy types, calls decide(), and writes
        the result to the Library if committed.

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

        # Step 0 (14-1-4): Dedup check — skip before any LLM call
        if self.dedup is not None and self.dedup.contains(event.event_id):
            logger.info(
                "Dedup hit: event_id=%s already processed, skipping",
                event.event_id,
            )
            return DecisionResult(
                reason="dedup_skipped",
                committed=False,
                event_id=event.event_id,
                locale=event.locale if event.locale in self.locales else "en",
            )

        # Guard: llm_fn must be set
        if self.llm_fn is None:
            logger.error(
                "llm_fn not configured — cannot process event %s",
                event.event_id,
            )
            return DecisionResult(
                reason="decision_error",
                committed=False,
                event_id=event.event_id,
                locale=event.locale or "en",
            )

        # Step 1: Convert proto segment to policy types
        segment = self._convert_segment(event)

        # Step 2: Build library state from filesystem
        library_state = self._build_library_state()

        # Step 3: Determine effective locale (fallback to "en")
        effective_locale = event.locale if event.locale in self.locales else "en"

        # Step 4: Call decide()
        try:
            policy_result: PolicyDecisionResult = decide(
                segment=segment,
                library_state=library_state,
                locale=effective_locale,
                llm_fn=self.llm_fn,
            )
        except Exception:
            logger.exception(
                "decide() raised an exception for event %s",
                event.event_id,
            )
            return DecisionResult(
                reason="decision_error",
                committed=False,
                event_id=event.event_id,
                locale=effective_locale,
            )

        # Step 5: If committed, write page and commit to git
        commit_hash = None
        if policy_result.committed:
            commit_hash = self._write_and_commit(policy_result, event)
            logger.info(
                "Page written: event_id=%s reason=%s page_path=%s",
                event.event_id,
                policy_result.reason,
                policy_result.page_path,
            )
        else:
            logger.info(
                "Event skipped: event_id=%s reason=%s",
                event.event_id,
                policy_result.reason,
            )

        # Step 6 (14-1-4): Record in dedup cache after successful processing
        if self.dedup is not None:
            self.dedup.add(event.event_id)
            self.dedup.save()

        # Step 7: Bridge policy DecisionResult to loop DecisionResult
        return DecisionResult(
            reason=policy_result.reason,
            committed=policy_result.committed,
            event_id=event.event_id,
            locale=effective_locale,
            page_path=policy_result.page_path if policy_result.committed else None,
            content=policy_result.content if policy_result.committed else None,
        )
