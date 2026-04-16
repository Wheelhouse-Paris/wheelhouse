"""LRU dedup cache for idempotent event processing.

Prevents duplicate Library writes when the same event_id is delivered
more than once (at-least-once delivery from ZMQ/SQS). Persisted to a
JSON file on the Library volume so the cache survives restarts.

Story: 14-1-4
NFR: NFR10 (1000-event dedup cache), NFR9 (crash recovery)
"""

from __future__ import annotations

import json
import logging
import os
from collections import OrderedDict
from pathlib import Path

logger = logging.getLogger("wheelhouse.librarian")

_DEFAULT_MAXSIZE = 1000
_SCHEMA_VERSION = 1


class DedupCache:
    """LRU dedup cache backed by an OrderedDict.

    Thread-safety: not required — LibrarianLoop is single-threaded (ADR-046).

    Args:
        maxsize: Maximum number of event IDs to retain. Oldest evicted first.
        path: Filesystem path for JSON persistence. None disables persistence.
    """

    def __init__(
        self,
        maxsize: int = _DEFAULT_MAXSIZE,
        path: Path | None = None,
    ) -> None:
        self._cache: OrderedDict[str, None] = OrderedDict()
        self._maxsize = maxsize
        self._path = path

    # ── Public API ────────────────────────────────────────────────────

    def contains(self, event_id: str) -> bool:
        """Check if event_id is in the cache.

        If present, the entry is promoted to most-recently-used (LRU touch).
        """
        if event_id in self._cache:
            self._cache.move_to_end(event_id)
            return True
        return False

    def add(self, event_id: str) -> None:
        """Add event_id to the cache, evicting the oldest if at capacity."""
        if event_id in self._cache:
            self._cache.move_to_end(event_id)
            return
        self._cache[event_id] = None
        while len(self._cache) > self._maxsize:
            self._cache.popitem(last=False)

    def save(self, path: Path | None = None) -> None:
        """Persist cache to JSON. Atomic via write-to-tmp + os.replace().

        Args:
            path: Override the default path set at construction.
        """
        target = path or self._path
        if target is None:
            return

        data = {
            "version": _SCHEMA_VERSION,
            "event_ids": list(self._cache.keys()),
        }

        tmp = target.with_suffix(".tmp")
        try:
            tmp.write_text(json.dumps(data), encoding="utf-8")
            os.replace(str(tmp), str(target))
        except OSError:
            logger.warning("Failed to persist dedup cache to %s", target, exc_info=True)

    @property
    def size(self) -> int:
        """Current number of cached event IDs."""
        return len(self._cache)

    # ── Class Methods ─────────────────────────────────────────────────

    @classmethod
    def load(
        cls,
        path: Path,
        maxsize: int = _DEFAULT_MAXSIZE,
    ) -> DedupCache:
        """Load cache from a JSON file. Returns empty cache on any error.

        Args:
            path: Path to the .dedup JSON file.
            maxsize: Maximum cache size.
        """
        cache = cls(maxsize=maxsize, path=path)

        if not path.exists():
            logger.info("No dedup cache file at %s — starting with empty cache", path)
            return cache

        try:
            raw = path.read_text(encoding="utf-8")
            data = json.loads(raw)

            if not isinstance(data, dict) or "event_ids" not in data:
                raise ValueError("Invalid dedup cache format: missing 'event_ids' key")

            event_ids = data["event_ids"]
            if not isinstance(event_ids, list):
                raise ValueError("Invalid dedup cache format: 'event_ids' is not a list")

            # Restore in order (oldest first). Truncate to maxsize.
            for eid in event_ids[-maxsize:]:
                if isinstance(eid, str):
                    cache._cache[eid] = None

            logger.info(
                "Loaded dedup cache from %s: %d entries", path, cache.size
            )
        except (json.JSONDecodeError, ValueError, OSError) as exc:
            logger.warning(
                "Corrupt or unreadable dedup cache at %s — starting empty: %s",
                path,
                exc,
            )
            cache = cls(maxsize=maxsize, path=path)

        return cache
