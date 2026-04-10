"""Library retrieval skill — relevance-gated grep search (Story 13-14).

This module delivers the *retrieval core* for the ``library.search`` skill.
It does NOT register anything with the skill dispatch path (that is
13-15) and it does NOT emit a :class:`SkillResult` (13-15 or later wires
it into the registry with citations).

Retrieval model
---------------

Two-stage call, both injectable for tests:

1. :func:`relevance_gate` — a cheap LLM judgment over the user query
   (plus an optional domain hint) that answers the single yes/no
   question: *is this query plausibly about anything the Library knows?*
   If the gate closes, :func:`retrieve` returns ``None`` without
   touching the sandbox. This is the NFR1 budget preserver — 90% of
   chit-chat should never pay the I/O cost of a full scan. To keep the
   unit tests hermetic and to fail safe in environments where no LLM is
   bound, the default with ``llm_fn=None`` is **False** (gate closed).

2. :func:`search` — a pure grep-grade scan of every ``*.md`` page under
   the Library root. For each page:

   * Parse the YAML front-matter via the 13-16 hand-rolled parser so we
     pick up ``cross_refs`` without taking a PyYAML dependency.
   * Extract the display title from the first ``# …`` heading in the
     body (the 13-8 page template does not put ``title`` in
     front-matter; it lives in the body's H1).
   * Tokenize the query and score the page by weighted token-match
     counts: title × :data:`W_TITLE`, cross_refs × :data:`W_CROSS_REF`,
     body × :data:`W_BODY`.
   * Build a 200-char snippet centered on the first matched token
     offset in the body.

   Top-k matches by score are returned in a :class:`LibraryRetrievalResult`
   along with a ``total_pages_searched`` count and a monotonic-clock
   ``elapsed_ms`` so the caller can report on NFR1 compliance.

Grep, not embeddings
--------------------

Per **NFR13** the v1 retrieval layer is grep-based. Embedding search,
approximate nearest neighbor, and any form of persistent index live in
v2. A full scan of 500 ``.md`` pages with small bodies comfortably fits
under the **NFR1** 2-second budget on a modern laptop; we don't need
anything more sophisticated yet. The scanner is written to be easily
replaceable — callers depend on :func:`retrieve` / :func:`search` and
the two dataclasses, not on the internal scoring loop.

See:
    - _bmad-output/planning-artifacts/wh/epics-library.md FW-4.1 (FR17, NFR1, NFR13)
    - _bmad-output/planning-artifacts/wh/architecture.md
    - _bmad-output/implementation-artifacts/wh/13-14-relevance-gated-retrieval.md
"""

from __future__ import annotations

import dataclasses
import logging
import time
import unicodedata
from typing import Any, Callable, Mapping, Sequence

from wheelhouse.skills.library_lint import _parse_front_matter
from wheelhouse.skills.library_sandbox import LibrarySandbox

logger = logging.getLogger("wheelhouse.library_retrieval")


# ─── Scoring weights ──────────────────────────────────────────────────

#: Per-token weight applied to matches in the page title (first H1).
#: Title matches are the strongest relevance signal in a grep-grade
#: retriever — a user asking about "pricing" almost always means the
#: page whose title IS "Pricing".
W_TITLE = 5

#: Per-token weight applied to matches in the ``cross_refs`` front-matter
#: list. A page that is cross-referenced with the query term is a strong
#: secondary signal — it says "other pages thought this page was about
#: <token>".
W_CROSS_REF = 3

#: Per-token weight applied to matches anywhere in the page body. This
#: is the floor — a single body hit is always worth something, but any
#: title hit outranks any reasonable number of body hits on the same
#: token.
W_BODY = 1


#: Snippet cap. 200 chars is enough for a human-readable preview in
#: chat output without blowing the agent's context window on a long
#: match list.
SNIPPET_MAX_LEN = 200


# ─── Stopwords ────────────────────────────────────────────────────────

#: Minimal English stopword set scoped to the high-frequency terms that
#: would otherwise skew short-query scoring. Not exhaustive — NFR13
#: accepts grep-grade precision in v1. Extend carefully; every stopword
#: removed from a query silently weakens the match count for pages
#: containing that stopword.
_STOPWORDS: frozenset[str] = frozenset(
    {
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "by",
        "for",
        "from",
        "has",
        "have",
        "how",
        "i",
        "in",
        "is",
        "it",
        "its",
        "me",
        "my",
        "of",
        "on",
        "or",
        "that",
        "the",
        "this",
        "to",
        "was",
        "what",
        "when",
        "where",
        "who",
        "why",
        "will",
        "with",
        "you",
        "your",
    }
)


# ─── Dataclasses ──────────────────────────────────────────────────────


@dataclasses.dataclass(frozen=True)
class RetrievalMatch:
    """A single page match returned by :func:`search`.

    Attributes:
        slug: sandbox-relative page path (the canonical identifier —
            same convention as :class:`library_lint.LibraryPage.path`),
            e.g. ``"clients/acme.md"``.
        title: display title extracted from the page's first H1
            heading, falling back to the slug if no heading is present.
        snippet: at most :data:`SNIPPET_MAX_LEN` characters of body
            text centered on the first matched token. Intended for
            rendering in agent responses; 13-15 may polish presentation.
        score: floating-point relevance score. Higher is better.
            Meaningful only for relative ordering within a single
            :class:`LibraryRetrievalResult` — do not compare scores
            across different queries.
    """

    slug: str
    title: str
    snippet: str
    score: float


@dataclasses.dataclass(frozen=True)
class LibraryRetrievalResult:
    """The result of a single :func:`search` call.

    Attributes:
        matches: top-k :class:`RetrievalMatch` list, sorted by score
            descending. Ties are broken by slug lexicographic order for
            determinism.
        query: the original query string (echoed so the caller can
            round-trip it into a :class:`SkillResult` without re-passing).
        total_pages_searched: count of ``*.md`` pages actually read
            and scored during this call. Excludes the ``index.md`` TOC
            and any ``.git/*`` entries ``sandbox.list()`` returns.
        elapsed_ms: wall-clock time from the start of the scan to the
            return, in integer milliseconds. Caller (13-15, 13-18)
            uses this to surface NFR1 compliance in logs.
    """

    matches: list[RetrievalMatch]
    query: str
    total_pages_searched: int
    elapsed_ms: int


# ─── Tokenizer ────────────────────────────────────────────────────────


def _ascii_fold(text: str) -> str:
    """Strip diacritics via NFKD normalization for tokenizer robustness."""
    normalized = unicodedata.normalize("NFKD", text)
    return "".join(ch for ch in normalized if not unicodedata.combining(ch))


def tokenize(text: str) -> list[str]:
    """Tokenize a string for grep-grade matching.

    Pipeline:

    1. Lowercase.
    2. ASCII-fold (diacritic-strip) so ``café`` and ``cafe`` match.
    3. Split on any run of non-alphanumeric characters — this is the
       "grep" part: punctuation, whitespace, and markdown syntax all
       become token boundaries.
    4. Drop :data:`_STOPWORDS` so short queries like "what is the acme
       plan" score against the signal tokens ``acme`` and ``plan``.

    The returned list preserves order for downstream uses (snippet
    anchoring, specifically) but may contain duplicates — callers that
    need a set should dedupe explicitly.
    """
    folded = _ascii_fold(text.lower())
    tokens: list[str] = []
    buf: list[str] = []
    for ch in folded:
        if ch.isalnum():
            buf.append(ch)
        else:
            if buf:
                tok = "".join(buf)
                buf = []
                if tok and tok not in _STOPWORDS:
                    tokens.append(tok)
    if buf:
        tok = "".join(buf)
        if tok and tok not in _STOPWORDS:
            tokens.append(tok)
    return tokens


# ─── Title extraction ─────────────────────────────────────────────────


def _extract_title(body: str, fallback: str) -> str:
    """Pull the display title from the first markdown H1 in the body.

    Walks lines looking for the first line that starts with ``#`` but
    not ``##``. Returns the text after the hash(es) trimmed. If no H1
    is present, returns ``fallback`` (the slug) so the match always has
    a displayable identifier.
    """
    for line in body.splitlines():
        stripped = line.strip()
        if not stripped.startswith("#"):
            continue
        # Count leading hashes.
        i = 0
        while i < len(stripped) and stripped[i] == "#":
            i += 1
        if i == 0:
            continue
        # H1 only — a line beginning with "## " is not the title.
        if i != 1:
            continue
        rest = stripped[i:].strip()
        if rest:
            return rest
    return fallback


# ─── Snippet builder ──────────────────────────────────────────────────


def _build_snippet(body: str, query_tokens: Sequence[str]) -> str:
    """Build a short preview centered on the first matched token.

    Lowercases and ASCII-folds the body for the anchor search (so an
    accented page still matches a plain-ASCII query), but returns the
    slice from the ORIGINAL body so the rendered snippet keeps its
    diacritics and casing. If no query token is found, returns the
    first :data:`SNIPPET_MAX_LEN` chars of the body as a fallback —
    this only happens when the caller is scoring on cross_refs alone.
    """
    if not body:
        return ""
    if len(body) <= SNIPPET_MAX_LEN:
        return body

    folded = _ascii_fold(body.lower())
    anchor = -1
    for tok in query_tokens:
        if not tok:
            continue
        idx = folded.find(tok)
        if idx != -1:
            anchor = idx
            break

    if anchor < 0:
        return body[:SNIPPET_MAX_LEN]

    half = SNIPPET_MAX_LEN // 2
    start = max(0, anchor - half)
    end = min(len(body), start + SNIPPET_MAX_LEN)
    # If the window ran off the end, shift the start back to keep the
    # snippet as close to max length as possible.
    if end - start < SNIPPET_MAX_LEN:
        start = max(0, end - SNIPPET_MAX_LEN)
    return body[start:end]


# ─── Page scoring ─────────────────────────────────────────────────────


def _count_token_hits(haystack_tokens: Sequence[str], query_tokens: Sequence[str]) -> int:
    """Count how many (token, position) pairs in the haystack match any query token.

    Simple O(n*m) with the expectation that both sides are small (tens
    to low hundreds of tokens per page, 1–10 tokens per query).
    """
    if not query_tokens:
        return 0
    query_set = set(query_tokens)
    return sum(1 for t in haystack_tokens if t in query_set)


def _score_page(
    *,
    title: str,
    body: str,
    cross_refs: Sequence[str],
    query_tokens: Sequence[str],
) -> float:
    """Compute the weighted score for one page against one query."""
    title_hits = _count_token_hits(tokenize(title), query_tokens)
    body_hits = _count_token_hits(tokenize(body), query_tokens)
    # Each cross_ref entry is tokenized independently so a ref like
    # ``clients/acme-corp.md`` contributes both ``clients`` and ``acme``
    # and ``corp`` to the match count.
    ref_tokens: list[str] = []
    for ref in cross_refs:
        ref_tokens.extend(tokenize(ref))
    cross_ref_hits = _count_token_hits(ref_tokens, query_tokens)

    return (
        float(title_hits) * W_TITLE
        + float(cross_ref_hits) * W_CROSS_REF
        + float(body_hits) * W_BODY
    )


# ─── Page iteration ───────────────────────────────────────────────────


def _is_skippable(rel_path: str) -> bool:
    """True if ``rel_path`` should never be scored by retrieval.

    Skips:

    * ``index.md`` — the table of contents. It's useful for navigation
      but its body is a flat link list that would spuriously boost the
      score of any generic term. 13-15 may render it separately as a
      "see also" helper.
    * Anything under ``.git/`` — git internals leak through
      ``sandbox.list()`` because the sandbox lists the raw directory
      tree; they're not Library content.
    * Non-``.md`` files — assets, binaries, etc.
    """
    if rel_path == "index.md":
        return True
    if rel_path.startswith(".git/") or "/.git/" in rel_path:
        return True
    if not rel_path.endswith(".md"):
        return True
    return False


def _iter_pages(sandbox: LibrarySandbox) -> list[str]:
    """Return the list of scorable page paths under the sandbox root."""
    return [rel for rel in sandbox.list(".") if not _is_skippable(rel)]


# ─── Public: search ──────────────────────────────────────────────────


def search(
    sandbox: LibrarySandbox,
    query: str,
    max_results: int = 5,
    domain_hint: str | None = None,
) -> LibraryRetrievalResult:
    """Run a grep-grade scan of the Library and return the top-k matches.

    ``domain_hint`` is accepted for forward-compatibility with 13-15 /
    13-18 (an optional topic label from the agent — e.g. ``"sales crm"``
    — which a future scorer may use to re-rank). In v1 it is recorded
    but does NOT affect the score; the code path remains pure grep.

    The function is defensive: a page that fails to read or parse is
    logged at DEBUG and skipped rather than crashing the whole scan.
    """
    _ = domain_hint  # reserved for v1.1 re-ranking hook
    query_tokens = tokenize(query)
    start = time.monotonic()

    pages = _iter_pages(sandbox)
    scored: list[RetrievalMatch] = []

    for rel in pages:
        try:
            raw = sandbox.read(rel)
        except Exception as exc:  # pragma: no cover - defensive
            logger.debug("library_retrieval: failed to read %s: %s", rel, exc)
            continue

        front_matter, body = _parse_front_matter(raw)
        cross_refs = _extract_cross_refs(front_matter)
        title = _extract_title(body, fallback=rel)

        score = _score_page(
            title=title,
            body=body,
            cross_refs=cross_refs,
            query_tokens=query_tokens,
        )
        if score <= 0:
            continue

        snippet = _build_snippet(body, query_tokens)
        scored.append(
            RetrievalMatch(
                slug=rel,
                title=title,
                snippet=snippet,
                score=score,
            )
        )

    # Sort by score desc, ties broken by slug ascending for determinism.
    scored.sort(key=lambda m: (-m.score, m.slug))
    top = scored[:max_results]

    elapsed_ms = int((time.monotonic() - start) * 1000)
    return LibraryRetrievalResult(
        matches=top,
        query=query,
        total_pages_searched=len(pages),
        elapsed_ms=elapsed_ms,
    )


def _extract_cross_refs(front_matter: Mapping[str, Any]) -> list[str]:
    """Coerce the front-matter ``cross_refs`` field into a list of strings."""
    raw = front_matter.get("cross_refs", [])
    if isinstance(raw, str):
        return [raw]
    if not isinstance(raw, list):
        return []
    return [str(r) for r in raw if r]


# ─── Public: relevance_gate ──────────────────────────────────────────


#: Callable shape for the injected relevance-gate LLM function. Takes
#: the query and an optional domain hint, returns a boolean:
#: True == "Library-relevant, go ahead and retrieve".
RelevanceGateFn = Callable[[str, "str | None"], bool]


def relevance_gate(
    query: str,
    domain_hint: str | None = None,
    llm_fn: RelevanceGateFn | None = None,
) -> bool:
    """Decide whether the query warrants touching the Library.

    This is a **pure LLM judgment**, not a grep. We deliberately keep
    the decision out of any string-matching heuristic because the whole
    point of the gate is to catch queries whose tokens don't obviously
    overlap the Library (e.g. "do we have the numbers Alice asked
    about?" where "numbers" and "Alice" may or may not be Library-y).

    ``llm_fn`` signature: ``Callable[[str, str | None], bool]`` — called
    exactly once per :func:`relevance_gate` call with ``(query,
    domain_hint)``. Injected so unit tests can stub it.

    **Safe default**: when ``llm_fn is None``, return ``False``. This
    means a caller that forgot to bind an LLM gets zero-cost retrieval
    skipping instead of a free full-library scan on every chit-chat
    message. The production wiring (13-15 / 13-18) MUST supply an
    ``llm_fn``.
    """
    if llm_fn is None:
        logger.debug(
            "library_retrieval: relevance_gate called with no llm_fn — "
            "defaulting to False (skip retrieval)"
        )
        return False
    try:
        return bool(llm_fn(query, domain_hint))
    except Exception as exc:  # pragma: no cover - defensive
        logger.warning(
            "library_retrieval: relevance_gate llm_fn raised %s — "
            "treating as gate closed",
            exc,
        )
        return False


# ─── Public: retrieve ────────────────────────────────────────────────


def retrieve(
    sandbox: LibrarySandbox,
    query: str,
    max_results: int = 5,
    domain_hint: str | None = None,
    llm_fn: RelevanceGateFn | None = None,
) -> LibraryRetrievalResult | None:
    """Relevance-gated entry point for Library retrieval.

    Calls :func:`relevance_gate` first. If the gate closes, returns
    ``None`` immediately — **no** ``sandbox.list()`` and **no**
    ``sandbox.read()`` calls are made, preserving the NFR1 budget for
    the 90% of messages that are chit-chat. If the gate opens, delegates
    to :func:`search` with the same arguments.

    Callers that want to force a scan (e.g. 13-25 ``wh library list``
    or an operator CLI) should call :func:`search` directly. This
    function is the conversation-path entry point.
    """
    if not relevance_gate(query, domain_hint=domain_hint, llm_fn=llm_fn):
        return None
    return search(
        sandbox,
        query,
        max_results=max_results,
        domain_hint=domain_hint,
    )


__all__ = [
    "LibraryRetrievalResult",
    "RelevanceGateFn",
    "RetrievalMatch",
    "SNIPPET_MAX_LEN",
    "W_BODY",
    "W_CROSS_REF",
    "W_TITLE",
    "relevance_gate",
    "retrieve",
    "search",
    "tokenize",
]
