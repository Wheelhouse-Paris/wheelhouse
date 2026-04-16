"""LibrarianLoop — event consumer and decision dispatch.

Implements the decision loop from ADR-046. Converts proto types to
policy types, calls decide(), writes pages via LibrarySandbox, and
commits to git with structured metadata.

Stories:
  - 14-1-2: skeleton (process_event placeholder)
  - 14-1-3: full decision policy wiring
  - 14-1-4: idempotent event processing with LRU dedup cache
  - 14-1-5: structured JSON decision logging, step_spans, commit hash,
             LLM timeout, SkillResult emission
"""

from __future__ import annotations

import concurrent.futures
import datetime
import logging
import subprocess
import time
from pathlib import Path
from typing import Any, Callable

from wheelhouse.librarian.dedup import DedupCache
from wheelhouse.librarian.decision_log import (
    DecisionLogEntry,
    SpanTimer,
    build_snippet,
    emit_decision_log,
)
from wheelhouse.librarian.proto import LibraryWriteEvent
from wheelhouse.librarian.types import DecisionResult
from wheelhouse.skills.library_sandbox import LibrarySandbox
from wheelhouse.skills.librarian.decide import decide
from wheelhouse.skills.librarian.types import (
    ConversationMessage as PolicyConversationMessage,
    DecisionResult as PolicyDecisionResult,
    LibraryState,
)

logger = logging.getLogger("wheelhouse.librarian")

# LLM call timeout in seconds (AC-3: decision_timeout after 10s)
_LLM_TIMEOUT_S = 10


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
        publish_skill_result: Optional async callback to emit SkillResult
                              for metering (story 14-1-5, AC-6).
        agent_name: Librarian agent identity for SkillResult attribution.
    """

    def __init__(
        self,
        library_path: str | Path,
        library_id: str,
        locales: list[str] | None = None,
        llm_fn: Callable[[str, str], str] | None = None,
        sandbox: LibrarySandbox | None = None,
        dedup: DedupCache | None = None,
        publish_skill_result: Callable[..., Any] | None = None,
        agent_name: str = "",
    ) -> None:
        self.library_path = Path(library_path)
        self.library_id = library_id
        self.locales = locales or ["en", "fr"]
        self.llm_fn = llm_fn
        self.sandbox = sandbox
        self.dedup = dedup
        self.publish_skill_result = publish_skill_result
        self.agent_name = agent_name

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

    def _get_commit_hash(self) -> str | None:
        """Extract the HEAD commit hash after a successful git commit.

        Uses ``git rev-parse HEAD`` in the library path. Returns None
        on any failure (best-effort — the commit already succeeded).
        """
        try:
            result = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=str(self.library_path),
                capture_output=True,
                text=True,
                timeout=5,
            )
            if result.returncode == 0:
                return result.stdout.strip()
        except Exception:
            logger.debug("Failed to extract commit hash", exc_info=True)
        return None

    def _write_and_commit(
        self,
        policy_result: PolicyDecisionResult,
        event: LibraryWriteEvent,
        timer: SpanTimer,
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

        # Transaction — include source_agent_id in summary for
        # multi-agent attribution (story 14-2-4, FR22, FR40).
        source = event.source_agent_id or "unknown"

        try:
            with self.sandbox.transaction(
                operation="librarian_decide",
                summary=f"{reason}: {page_path} (source: {source})",
            ) as txn:
                # Write page
                self.sandbox.write(page_path, content)

                # Commit metadata — sources carries the originating agent
                # so git log shows attribution (ADR-039).
                if is_update:
                    txn.commit_metadata["pages_updated"] = [page_path]
                else:
                    txn.commit_metadata["pages_created"] = [page_path]
                txn.commit_metadata["sources"] = [source]

                with timer.span("git_commit"):
                    pass  # commit happens on transaction __exit__
        except Exception:
            logger.exception(
                "Failed to write/commit page %s for event %s",
                page_path,
                event.event_id,
            )
            return None

        # Extract commit hash after successful commit (14-1-5, AC-1).
        return self._get_commit_hash()

    def _emit_log_entry(
        self,
        event: LibraryWriteEvent,
        result: DecisionResult,
    ) -> None:
        """Emit a structured JSON decision log entry (14-1-5, FR28)."""
        # Build snippet from segment content (NFR20: <=200 chars).
        segment_texts = [msg.content for msg in event.segment]
        snippet = build_snippet(segment_texts)

        entry = DecisionLogEntry(
            schema_version=result.schema_version,
            event_id=result.event_id,
            library_id=self.library_id,
            source_agent_id=event.source_agent_id,
            conversation_id=event.conversation_id,
            reason=result.reason,
            committed=result.committed,
            commit_hash=result.commit_hash,
            locale=result.locale,
            tokens_consumed=result.tokens_consumed,
            snippet=snippet,
            timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
            duration_ms=result.duration_ms,
            step_spans=result.step_spans,
        )
        emit_decision_log(entry)

    def _emit_skill_result(
        self,
        event: LibraryWriteEvent,
        result: DecisionResult,
    ) -> None:
        """Emit a SkillResult for metering (14-1-5, AC-6).

        The callback is provided by __main__.py and publishes the
        SkillResult to the broker asynchronously.
        """
        if self.publish_skill_result is None:
            return

        try:
            self.publish_skill_result(
                invocation_id=event.event_id,
                skill_name="librarian_decide",
                success=result.committed,
                output=result.reason,
                tokens_consumed=result.tokens_consumed,
                library_id=self.library_id,
                agent_id=self.agent_name,
            )
        except Exception:
            logger.debug("Failed to emit SkillResult", exc_info=True)

    def process_event(self, event: LibraryWriteEvent) -> DecisionResult:
        """Process a single LibraryWriteEvent through the decision policy.

        Converts proto types to policy types, calls decide(), and writes
        the result to the Library if committed. Emits a structured JSON
        decision log entry and SkillResult for every decision.

        Args:
            event: Deserialized LibraryWriteEvent from the broker.

        Returns:
            DecisionResult with the processing outcome.
        """
        overall_start = time.monotonic()
        timer = SpanTimer()

        logger.info(
            "Processing event: event_id=%s source_agent_id=%s library_id=%s locale=%s",
            event.event_id,
            event.source_agent_id,
            event.library_id,
            event.locale,
        )

        # Step 0 (14-1-4): Dedup check — skip before any LLM call
        with timer.span("dedup_check"):
            is_dup = self.dedup is not None and self.dedup.contains(event.event_id)

        if is_dup:
            logger.info(
                "Dedup hit: event_id=%s already processed, skipping",
                event.event_id,
            )
            duration_ms = int((time.monotonic() - overall_start) * 1000)
            result = DecisionResult(
                reason="dedup_skipped",
                committed=False,
                event_id=event.event_id,
                locale=event.locale if event.locale in self.locales else "en",
                duration_ms=duration_ms,
                step_spans=timer.spans,
            )
            self._emit_log_entry(event, result)
            self._emit_skill_result(event, result)
            return result

        # Guard: llm_fn must be set
        if self.llm_fn is None:
            logger.error(
                "llm_fn not configured — cannot process event %s",
                event.event_id,
            )
            duration_ms = int((time.monotonic() - overall_start) * 1000)
            result = DecisionResult(
                reason="decision_error",
                committed=False,
                event_id=event.event_id,
                locale=event.locale or "en",
                duration_ms=duration_ms,
                step_spans=timer.spans,
            )
            self._emit_log_entry(event, result)
            self._emit_skill_result(event, result)
            return result

        # Step 1: Convert proto segment to policy types
        segment = self._convert_segment(event)

        # Step 2: Build library state from filesystem
        with timer.span("prompt_load"):
            library_state = self._build_library_state()

        # Step 3: Determine effective locale (fallback to "en")
        effective_locale = event.locale if event.locale in self.locales else "en"

        # Step 4: Call decide() with timeout (AC-3: 10s timeout)
        try:
            with timer.span("llm_call"):
                policy_result: PolicyDecisionResult = self._call_decide_with_timeout(
                    segment=segment,
                    library_state=library_state,
                    locale=effective_locale,
                )
        except concurrent.futures.TimeoutError:
            logger.warning(
                "LLM call timed out after %ds for event %s",
                _LLM_TIMEOUT_S,
                event.event_id,
            )
            duration_ms = int((time.monotonic() - overall_start) * 1000)
            result = DecisionResult(
                reason="decision_timeout",
                committed=False,
                event_id=event.event_id,
                locale=effective_locale,
                duration_ms=duration_ms,
                step_spans=timer.spans,
            )
            self._emit_log_entry(event, result)
            self._emit_skill_result(event, result)
            return result
        except Exception:
            logger.exception(
                "decide() raised an exception for event %s",
                event.event_id,
            )
            duration_ms = int((time.monotonic() - overall_start) * 1000)
            result = DecisionResult(
                reason="decision_error",
                committed=False,
                event_id=event.event_id,
                locale=effective_locale,
                duration_ms=duration_ms,
                step_spans=timer.spans,
            )
            self._emit_log_entry(event, result)
            self._emit_skill_result(event, result)
            return result

        # Step 5: If committed, write page and commit to git
        commit_hash = None
        if policy_result.committed:
            commit_hash = self._write_and_commit(policy_result, event, timer)
            logger.info(
                "Page written: event_id=%s reason=%s page_path=%s commit_hash=%s",
                event.event_id,
                policy_result.reason,
                policy_result.page_path,
                commit_hash,
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

        # Step 7: Build final DecisionResult with all fields
        duration_ms = int((time.monotonic() - overall_start) * 1000)
        result = DecisionResult(
            reason=policy_result.reason,
            committed=policy_result.committed,
            event_id=event.event_id,
            locale=effective_locale,
            page_path=policy_result.page_path if policy_result.committed else None,
            content=policy_result.content if policy_result.committed else None,
            commit_hash=commit_hash,
            duration_ms=duration_ms,
            step_spans=timer.spans,
        )

        # Step 8 (14-1-5): Emit structured decision log and SkillResult
        self._emit_log_entry(event, result)
        self._emit_skill_result(event, result)

        return result

    def _call_decide_with_timeout(
        self,
        segment: list[PolicyConversationMessage],
        library_state: LibraryState,
        locale: str,
    ) -> PolicyDecisionResult:
        """Call decide() with a configurable timeout.

        Uses a thread pool executor to enforce the 10-second timeout
        (AC-3). The llm_fn is synchronous, so we run decide() in a
        thread and wait with a timeout.

        Raises:
            concurrent.futures.TimeoutError: If the call exceeds _LLM_TIMEOUT_S.
        """
        assert self.llm_fn is not None  # Caller already guards this

        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as executor:
            future = executor.submit(
                decide,
                segment=segment,
                library_state=library_state,
                locale=locale,
                llm_fn=self.llm_fn,
            )
            return future.result(timeout=_LLM_TIMEOUT_S)
