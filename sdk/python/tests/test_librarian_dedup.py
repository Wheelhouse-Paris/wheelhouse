"""Tests for idempotent event processing and dedup cache (story 14-1-4).

Covers:
  - AC-1: Duplicate event skipped immediately (no LLM call)
  - AC-2: LRU eviction at 1000 entries
  - AC-3: Cache persistence across restarts (JSON save/load)
  - AC-4: Corrupt or missing .dedup file handled gracefully
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from wheelhouse.librarian.dedup import DedupCache
from wheelhouse.librarian.loop import LibrarianLoop
from wheelhouse.librarian.proto import (
    ConversationMessage as ProtoConversationMessage,
    LibraryWriteEvent,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_event(
    *,
    event_id: str = "evt-001",
    source_agent_id: str = "agent-a",
    library_id: str = "lib-test",
    conversation_id: str = "conv-001",
    locale: str = "en",
    segment_text: str = "My birthday is April 5th.",
) -> LibraryWriteEvent:
    """Create a minimal LibraryWriteEvent for testing."""
    event = LibraryWriteEvent()
    event.event_id = event_id
    event.source_agent_id = source_agent_id
    event.library_id = library_id
    event.conversation_id = conversation_id
    event.locale = locale
    msg = ProtoConversationMessage()
    msg.role = "user"
    msg.content = segment_text
    msg.timestamp_ms = 1713000000000
    event.segment = [msg]
    event.timestamp_ms = 1713000000000
    return event


def _mock_llm_response(
    reason: str,
    committed: bool,
    page_path: str | None = None,
    content: str | None = None,
) -> str:
    return json.dumps(
        {
            "reason": reason,
            "committed": committed,
            "page_path": page_path,
            "content": content,
        }
    )


class _TxnHandle:
    def __init__(self) -> None:
        self.commit_metadata: dict = {}

class _FakeTxnCtx:
    def __init__(self, sandbox: MagicMock) -> None:
        self._sandbox = sandbox
        self.handle = _TxnHandle()
    def __enter__(self) -> _TxnHandle:
        return self.handle
    def __exit__(self, exc_type, exc, tb) -> None:  # type: ignore[no-untyped-def]
        if exc_type is None:
            self._sandbox.commit(**self.handle.commit_metadata)

def _make_sandbox_mock() -> MagicMock:
    sandbox = MagicMock()
    sandbox.exists.return_value = False
    sandbox.list.return_value = []
    sandbox.read.return_value = ""
    sandbox.transaction.side_effect = lambda **kw: _FakeTxnCtx(sandbox)
    return sandbox


# ===========================================================================
# DedupCache unit tests
# ===========================================================================


class TestDedupCacheContains:
    """Test DedupCache.contains() and LRU touch behavior."""

    def test_empty_cache_returns_false(self) -> None:
        cache = DedupCache(maxsize=10)
        assert cache.contains("evt-001") is False

    def test_added_event_is_found(self) -> None:
        cache = DedupCache(maxsize=10)
        cache.add("evt-001")
        assert cache.contains("evt-001") is True

    def test_contains_promotes_to_mru(self) -> None:
        """Accessing an entry via contains() makes it most-recently-used."""
        cache = DedupCache(maxsize=3)
        cache.add("a")
        cache.add("b")
        cache.add("c")

        # Touch "a" — it should become MRU
        cache.contains("a")

        # Adding a new entry should evict "b" (now oldest), not "a"
        cache.add("d")
        assert cache.contains("a") is True
        assert cache.contains("b") is False
        assert cache.contains("c") is True
        assert cache.contains("d") is True


class TestDedupCacheAdd:
    """Test DedupCache.add() and LRU eviction."""

    def test_add_increments_size(self) -> None:
        cache = DedupCache(maxsize=10)
        cache.add("evt-001")
        assert cache.size == 1
        cache.add("evt-002")
        assert cache.size == 2

    def test_duplicate_add_does_not_increase_size(self) -> None:
        cache = DedupCache(maxsize=10)
        cache.add("evt-001")
        cache.add("evt-001")
        assert cache.size == 1

    def test_lru_eviction_at_maxsize(self) -> None:
        """AC-2: Oldest entry evicted when cache exceeds maxsize."""
        cache = DedupCache(maxsize=3)
        cache.add("a")
        cache.add("b")
        cache.add("c")
        assert cache.size == 3

        # Adding 4th evicts "a"
        cache.add("d")
        assert cache.size == 3
        assert cache.contains("a") is False
        assert cache.contains("b") is True
        assert cache.contains("d") is True

    def test_lru_eviction_at_1000(self) -> None:
        """AC-2: Full-scale test with 1000 entries."""
        cache = DedupCache(maxsize=1000)
        for i in range(1000):
            cache.add(f"evt-{i:04d}")
        assert cache.size == 1000

        # 1001st entry evicts evt-0000
        cache.add("evt-1000")
        assert cache.size == 1000
        assert cache.contains("evt-0000") is False
        assert cache.contains("evt-0001") is True
        assert cache.contains("evt-1000") is True


class TestDedupCachePersistence:
    """Test JSON save/load persistence."""

    def test_save_load_roundtrip(self, tmp_path: Path) -> None:
        """AC-3: Cache persists and restores across restarts."""
        dedup_file = tmp_path / ".dedup"

        # Create and populate
        cache = DedupCache(maxsize=10, path=dedup_file)
        cache.add("evt-001")
        cache.add("evt-002")
        cache.add("evt-003")
        cache.save()

        assert dedup_file.exists()

        # Load from file
        restored = DedupCache.load(dedup_file, maxsize=10)
        assert restored.size == 3
        assert restored.contains("evt-001") is True
        assert restored.contains("evt-002") is True
        assert restored.contains("evt-003") is True

    def test_save_preserves_lru_order(self, tmp_path: Path) -> None:
        """LRU order is preserved across save/load."""
        dedup_file = tmp_path / ".dedup"

        cache = DedupCache(maxsize=3, path=dedup_file)
        cache.add("a")
        cache.add("b")
        cache.add("c")
        # Touch "a" to make it MRU
        cache.contains("a")
        cache.save()

        restored = DedupCache.load(dedup_file, maxsize=3)
        # Adding new entry should evict "b" (oldest), not "a" (MRU)
        restored.add("d")
        assert restored.contains("a") is True
        assert restored.contains("b") is False

    def test_json_format(self, tmp_path: Path) -> None:
        """Verify the JSON schema on disk."""
        dedup_file = tmp_path / ".dedup"
        cache = DedupCache(maxsize=10, path=dedup_file)
        cache.add("evt-001")
        cache.add("evt-002")
        cache.save()

        data = json.loads(dedup_file.read_text())
        assert data["version"] == 1
        assert data["event_ids"] == ["evt-001", "evt-002"]

    def test_missing_file_returns_empty_cache(self, tmp_path: Path) -> None:
        """AC-4: Missing .dedup file starts with empty cache."""
        dedup_file = tmp_path / ".dedup"
        assert not dedup_file.exists()

        cache = DedupCache.load(dedup_file)
        assert cache.size == 0

    def test_corrupt_json_returns_empty_cache(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture
    ) -> None:
        """AC-4: Corrupt JSON starts with empty cache and logs warning."""
        dedup_file = tmp_path / ".dedup"
        dedup_file.write_text("this is not valid json{{{")

        with caplog.at_level(logging.WARNING, logger="wheelhouse.librarian"):
            cache = DedupCache.load(dedup_file)

        assert cache.size == 0
        assert "Corrupt" in caplog.text or "unreadable" in caplog.text

    def test_invalid_format_returns_empty_cache(
        self, tmp_path: Path, caplog: pytest.LogCaptureFixture
    ) -> None:
        """AC-4: Valid JSON but missing event_ids key starts empty."""
        dedup_file = tmp_path / ".dedup"
        dedup_file.write_text(json.dumps({"foo": "bar"}))

        with caplog.at_level(logging.WARNING, logger="wheelhouse.librarian"):
            cache = DedupCache.load(dedup_file)

        assert cache.size == 0

    def test_load_truncates_to_maxsize(self, tmp_path: Path) -> None:
        """Loaded cache truncated to maxsize (keeps newest)."""
        dedup_file = tmp_path / ".dedup"
        event_ids = [f"evt-{i:04d}" for i in range(100)]
        dedup_file.write_text(
            json.dumps({"version": 1, "event_ids": event_ids})
        )

        cache = DedupCache.load(dedup_file, maxsize=10)
        assert cache.size == 10
        # Should keep the last 10 (newest)
        assert cache.contains("evt-0099") is True
        assert cache.contains("evt-0090") is True
        assert cache.contains("evt-0000") is False

    def test_save_none_path_is_noop(self) -> None:
        """save() with no path is a no-op."""
        cache = DedupCache(maxsize=10, path=None)
        cache.add("evt-001")
        cache.save()  # Should not raise

    def test_atomic_write(self, tmp_path: Path) -> None:
        """Verify .dedup.tmp is not left behind after save."""
        dedup_file = tmp_path / ".dedup"
        cache = DedupCache(maxsize=10, path=dedup_file)
        cache.add("evt-001")
        cache.save()

        assert dedup_file.exists()
        assert not (tmp_path / ".dedup.tmp").exists()


# ===========================================================================
# Integration: DedupCache + LibrarianLoop
# ===========================================================================


class TestDedupInLibrarianLoop:
    """Test dedup integration in process_event()."""

    def test_duplicate_event_returns_dedup_skipped(self) -> None:
        """AC-1: Duplicate event_id returns immediately, no LLM call."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "durable_fact_written", True, "pages/test.md", "content"
            )
        )

        dedup = DedupCache(maxsize=100)

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            locales=["en", "fr"],
            llm_fn=llm_fn,
            sandbox=sandbox,
            dedup=dedup,
        )

        event = _make_event(event_id="evt-dup")

        # First call: processes normally
        result1 = loop.process_event(event)
        assert result1.reason == "durable_fact_written"
        assert result1.committed is True
        assert llm_fn.call_count == 1

        # Second call: dedup skipped, NO LLM call
        result2 = loop.process_event(event)
        assert result2.reason == "dedup_skipped"
        assert result2.committed is False
        assert result2.event_id == "evt-dup"
        assert llm_fn.call_count == 1  # NOT incremented

        # No additional sandbox interactions
        assert sandbox.transaction.call_count == 1

    def test_different_event_ids_both_processed(self) -> None:
        """Different event_ids are both processed normally."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "durable_fact_written", True, "pages/test.md", "content"
            )
        )
        dedup = DedupCache(maxsize=100)

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
            dedup=dedup,
        )

        result1 = loop.process_event(_make_event(event_id="evt-001"))
        result2 = loop.process_event(_make_event(event_id="evt-002"))

        assert result1.reason == "durable_fact_written"
        assert result2.reason == "durable_fact_written"
        assert llm_fn.call_count == 2

    def test_dedup_none_disables_check(self) -> None:
        """dedup=None disables dedup checking (backward compat)."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "durable_fact_written", True, "pages/test.md", "content"
            )
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
            dedup=None,  # Disabled
        )

        event = _make_event(event_id="evt-dup")

        # Both calls go through — no dedup
        result1 = loop.process_event(event)
        result2 = loop.process_event(event)
        assert result1.committed is True
        assert result2.committed is True
        assert llm_fn.call_count == 2

    def test_dedup_skipped_preserves_locale_fallback(self) -> None:
        """Dedup skip with unsupported locale falls back to 'en'."""
        dedup = DedupCache(maxsize=10)
        dedup.add("evt-de")

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            locales=["en", "fr"],
            llm_fn=MagicMock(),
            sandbox=_make_sandbox_mock(),
            dedup=dedup,
        )

        event = _make_event(event_id="evt-de", locale="de")
        result = loop.process_event(event)

        assert result.reason == "dedup_skipped"
        assert result.locale == "en"  # Fallback

    def test_skipped_event_not_cached(self) -> None:
        """Events that are skipped (committed=False) are still cached."""
        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response("transient_context", False)
        )
        dedup = DedupCache(maxsize=100)

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
            dedup=dedup,
        )

        event = _make_event(event_id="evt-skip")
        result1 = loop.process_event(event)
        assert result1.reason == "transient_context"
        assert dedup.contains("evt-skip") is True

        # Second call is deduped
        result2 = loop.process_event(event)
        assert result2.reason == "dedup_skipped"
        assert llm_fn.call_count == 1

    def test_dedup_save_called_after_processing(self, tmp_path: Path) -> None:
        """Dedup cache is saved after each event processing."""
        dedup_file = tmp_path / ".dedup"
        dedup = DedupCache(maxsize=100, path=dedup_file)

        sandbox = _make_sandbox_mock()
        llm_fn = MagicMock(
            return_value=_mock_llm_response(
                "durable_fact_written", True, "pages/test.md", "content"
            )
        )

        loop = LibrarianLoop(
            library_path="/tmp/test-lib",
            library_id="lib-test",
            llm_fn=llm_fn,
            sandbox=sandbox,
            dedup=dedup,
        )

        loop.process_event(_make_event(event_id="evt-persist"))

        # Verify the file was written
        assert dedup_file.exists()
        data = json.loads(dedup_file.read_text())
        assert "evt-persist" in data["event_ids"]
