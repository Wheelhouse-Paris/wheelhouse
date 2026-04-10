"""Library citation construction — FR18 helpers (Story 13-15).

When an agent answers a user from Library-retrieved content it MUST
cite both the Library page and the original source the page was
ingested from (FR18). Story 13-14 shipped the retrieval core which
returns :class:`RetrievalMatch` objects carrying ``slug``, ``title``,
``snippet``, and ``score`` — deliberately light on provenance so the
hot path does not pay a second disk read per match.

This module closes that gap. It exposes a small, pure-function helper
set that:

1. Re-reads each match's page via :meth:`LibrarySandbox.read`, parses
   the 13-8 YAML front-matter, and bundles the ``source`` / ``ingest_date``
   fields with the retrieval snippet into a :class:`Citation`.
2. Renders a list of :class:`Citation` objects in two shapes:
   a human-readable markdown footnote block and an inline
   ``[n]`` reference marker run.

The helpers are **synchronous** and side-effect-light: one
``sandbox.read`` per match. Matches whose page cannot be read (deleted
between the scan and the citation pass) are silently dropped rather
than raised — the caller would rather get N-1 citations than blow up
its entire response. Missing front-matter fields fall back to
``"unknown"`` / ``""`` so older or hand-authored pages still render.

Scope notes
-----------

- This module does NOT touch ``SkillRegistry`` or emit a
  :class:`SkillResult`. Conversation-path wiring is 13-18 or later.
- It does NOT dedupe. :func:`library_retrieval.search` returns unique
  page slugs, and any dedup policy upstream is the caller's business.
- It does NOT take a dependency on 13-13 (source provenance). The
  ``source`` field is a plain ``str``; when 13-13 extends the page
  front-matter with a structured provenance object, this module will
  stringify it at the boundary without changing the :class:`Citation`
  shape.
- It exposes sandbox-relative slugs only — never absolute paths
  (NFR9). The slugs come straight from
  :attr:`RetrievalMatch.slug` which 13-14 already enforces.

See:
    - _bmad-output/planning-artifacts/wh/epics-library.md FW-4 (FR18)
    - _bmad-output/implementation-artifacts/wh/13-15-citation-construction.md
    - sdk/python/wheelhouse/skills/library_retrieval.py (RetrievalMatch)
    - sdk/python/wheelhouse/skills/library_ingest.py (_render_page — front-matter format)
"""

from __future__ import annotations

import dataclasses
import logging
from typing import Iterable

from wheelhouse.skills.library_lint import _parse_front_matter
from wheelhouse.skills.library_retrieval import RetrievalMatch
from wheelhouse.skills.library_sandbox import LibrarySandbox

logger = logging.getLogger("wheelhouse.library_citations")


#: Fallback emitted for a page that lacks the ``source`` front-matter
#: key (older ingests or hand-authored pages). Kept as a module-level
#: constant so downstream renderers can special-case it if needed.
UNKNOWN_SOURCE = "unknown"


# ─── Dataclass ────────────────────────────────────────────────────────


@dataclasses.dataclass(frozen=True)
class Citation:
    """A single citation bundling the Library page and its original source.

    Attributes:
        page_slug: sandbox-relative page path (e.g. ``"clients/acme.md"``).
            Same convention as :attr:`RetrievalMatch.slug` — never an
            absolute filesystem path (NFR9).
        page_title: display title extracted from the page's H1 during
            retrieval. Echoed verbatim from :attr:`RetrievalMatch.title`.
        source: the original source the page was ingested from —
            ``source`` from the 13-8 front-matter. Falls back to
            :data:`UNKNOWN_SOURCE` when the field is missing.
        ingest_date: ISO-8601 timestamp string from the 13-8
            ``ingest_date`` front-matter field. Empty string when
            missing. Not parsed into a ``datetime`` — citations are
            pure-render values.
        snippet: the retrieval snippet echoed from
            :attr:`RetrievalMatch.snippet`. Capped at
            :data:`library_retrieval.SNIPPET_MAX_LEN` chars by the
            scanner.
    """

    page_slug: str
    page_title: str
    source: str
    ingest_date: str
    snippet: str


# ─── Front-matter extraction ──────────────────────────────────────────


def _coerce_str(value: object, default: str) -> str:
    """Stringify a front-matter field, falling back to ``default``.

    The 13-16 hand-rolled parser returns scalars as ``str`` already,
    but it may return a list for ``cross_refs`` or (after 13-13)
    arbitrary shapes for ``source``. We coerce defensively: any
    non-empty string is taken as-is, anything else (empty string,
    list, None, int) falls back to ``default``.
    """
    if isinstance(value, str) and value:
        return value
    return default


# ─── Public: build_citations ──────────────────────────────────────────


def build_citations(
    matches: Iterable[RetrievalMatch],
    sandbox: LibrarySandbox,
) -> list[Citation]:
    """Bundle each :class:`RetrievalMatch` with its page's source fields.

    For each match, re-reads the page via ``sandbox.read(match.slug)``,
    parses the 13-8 front-matter, and constructs a :class:`Citation`
    carrying ``source`` and ``ingest_date`` alongside the retrieval
    snippet. Matches whose page cannot be read are logged at DEBUG and
    **silently dropped** — the caller prefers a partial list to an
    exception in the middle of a response render.

    Args:
        matches: iterable of :class:`RetrievalMatch` from
            :func:`library_retrieval.search` or
            :func:`library_retrieval.retrieve`. Order is preserved in
            the returned list (typically this is score-descending).
        sandbox: the :class:`LibrarySandbox` the matches came from.
            Only ``.read()`` is called, once per match.

    Returns:
        A list of :class:`Citation` objects in the same order as
        ``matches``, minus any entry whose page read failed. An empty
        input yields an empty list without calling ``sandbox.read``.
    """
    citations: list[Citation] = []
    for match in matches:
        try:
            raw = sandbox.read(match.slug)
        except Exception as exc:
            logger.debug(
                "library_citations: dropping match %s — read failed: %s",
                match.slug,
                exc,
            )
            continue

        front_matter, _body = _parse_front_matter(raw)
        source = _coerce_str(front_matter.get("source"), UNKNOWN_SOURCE)
        ingest_date = _coerce_str(front_matter.get("ingest_date"), "")

        citations.append(
            Citation(
                page_slug=match.slug,
                page_title=match.title,
                source=source,
                ingest_date=ingest_date,
                snippet=match.snippet,
            )
        )
    return citations


# ─── Public: format_citations_markdown ────────────────────────────────


def format_citations_markdown(citations: list[Citation]) -> str:
    """Render a list of :class:`Citation` as a markdown footnote block.

    Each citation becomes a single line::

        [<n>] <page_title> (source: <source>, ingested: <ingest_date>)

    Numbered 1..N in list order, separated by ``\\n``. An empty list
    returns an empty string — callers can test the return directly
    without a length check.

    The renderer is deliberately minimal: no heading, no trailing
    newline, no per-line snippet. The agent surface can prepend its
    own ``### Sources`` header or post-process into HTML if the
    output channel supports it. Keeping the raw block tight makes
    downstream wrapping predictable.
    """
    if not citations:
        return ""
    lines: list[str] = []
    for i, c in enumerate(citations, start=1):
        lines.append(
            f"[{i}] {c.page_title} (source: {c.source}, ingested: {c.ingest_date})"
        )
    return "\n".join(lines)


# ─── Public: format_citations_inline ──────────────────────────────────


def format_citations_inline(citations: list[Citation]) -> str:
    """Render a list of :class:`Citation` as inline ``[n]`` markers.

    Returns ``"[1] [2] [3]"`` for three citations — a space-separated
    marker run the agent can drop next to a sentence that summarizes
    multiple sources. An empty list returns an empty string.

    The markers are deliberately content-free; the agent is expected
    to pair them with the corresponding
    :func:`format_citations_markdown` block further down in the
    response.
    """
    if not citations:
        return ""
    return " ".join(f"[{i}]" for i in range(1, len(citations) + 1))


__all__ = [
    "Citation",
    "UNKNOWN_SOURCE",
    "build_citations",
    "format_citations_markdown",
    "format_citations_inline",
]
