"""Library ingest skill — text / markdown / PDF (Stories 13-7, 13-8, 13-9).

This module registers the ``library_ingest`` skill with the Wheelhouse
Library skill dispatch path and delivers the invocation shell (13-7 —
parameter schema, plan-state gate, error-code catalogue), the text /
markdown content pipeline (13-8), and the PDF extraction branch (13-9)
that funnels extracted PDF text into the exact same summarizer +
transaction + writer code path as text/markdown.

Content pipeline (13-8 scope — text / markdown only):

  1. Validate ``source_type in {"text", "markdown"}``; ``"pdf"`` and
     ``"url"`` raise ``LIBRARY_INGEST_UNSUPPORTED_TYPE`` so 13-9 (PDF)
     and a later URL story can plug in without reshaping the seam.
  2. Resolve ``source_ref`` to UTF-8 text via either
     ``parameters["source_content"]`` (inline blob, used when the user
     pasted the content directly) or ``sandbox.read(source_ref)``
     (sandbox-relative path under ``.library/``). Path escape, missing
     file, and non-UTF-8 all collapse into
     ``LIBRARY_INGEST_SOURCE_NOT_FOUND`` / ``LIBRARY_INGEST_BINARY_REJECTED``
     with NO path echoed (NFR9).
  3. Call the injected summarizer (set via :func:`set_summarizer`) —
     agent-claude wires the real Claude call at boot, tests pass a
     fake. No summarizer wired → ``LIBRARY_INGEST_NO_SUMMARIZER``.
  4. Open a single ``sandbox.transaction("ingest", summary)`` context
     (ADR-039 one-commit-per-skill-invocation), write each drafted
     page with a minimal YAML front-matter block, update ``index.md``'s
     ``## Ingested sources`` section, and populate the handle's
     ``commit_metadata`` for the structured commit-message body.
  5. Return ``SkillResult(success=True, ...)`` with 13-27's library
     piggyback fields (``library_tokens``, ``library_page_count``,
     ``library_last_ingest_at``) populated.

Out of scope (deferred to sibling stories — do NOT add here):

  * PDF extraction — 13-9 adds a second branch inside
    :func:`_ingest_pipeline` for ``source_type == "pdf"`` that shells
    out to ``pdftotext`` and then calls the exact same summarizer /
    writer code delivered here.
  * Size / word-count / page-count limit enforcement — 13-10 adds a
    pre-summarizer gate.
  * Post-ingest cross-ref dangle validation — 13-11.
  * Source dedup on re-ingest — 13-12.
  * Richer provenance than a bare filename — 13-13.
  * ``SkillProgress`` streaming — 13-10.

See:
    - _bmad-output/planning-artifacts/wh/epics-library.md FW-3.1 / 3.2
    - _bmad-output/planning-artifacts/wh/architecture.md ADR-038 / 039
    - _bmad-output/implementation-artifacts/wh/13-7-library-ingest-skill-scaffolding.md
    - _bmad-output/implementation-artifacts/wh/13-8-text-markdown-ingest-with-llm-summarization.md
    - wheelhouse.errors.LibrarySkillError (code catalogue below)
"""

from __future__ import annotations

import dataclasses
import datetime as _dt
import inspect
import json
import logging
import os
from typing import Any, Callable, Mapping

from wheelhouse.errors import LibrarySkillError, PathEscapeError
from wheelhouse.skills.library_sandbox import LibrarySandbox
from wheelhouse.types import SkillResult

logger = logging.getLogger("wheelhouse.library_ingest")

# ─── Skill identity ───────────────────────────────────────────────────

#: Skill name string as it appears in ``SkillInvocation.skill_name``.
#: Snake-case single token per the existing ``SkillInvocation`` convention
#: — NOT dot-notation. 13-14..13-18 will register additional Library
#: skills (``library_retrieval``, ``library_lint``) with the same shape.
SKILL_NAME = "library_ingest"

#: Parameters the invocation MUST provide.
REQUIRED_PARAMS: tuple[str, ...] = ("source_type", "source_ref")

#: Parameters the invocation MAY provide.
#:
#: ``accept_large`` is a string-typed override for the FR11 50K-word
#: size gate (story 13-10). Accepted truthy values are ``"true"`` /
#: ``"True"`` / ``"1"`` / ``"yes"`` case-insensitively.
#:
#: ``allow_slug_reuse`` is a string-typed override for the story 13-11
#: post-ingest consistency gate's slug-collision check: when truthy,
#: a draft whose normalized path already exists in the Library is
#: treated as an intentional update rather than a hallucinated
#: re-summary. Story 13-12 (dedup) will replace this escape hatch with
#: proper source-driven update detection.
OPTIONAL_PARAMS: tuple[str, ...] = (
    "user_summary_hint",
    "accept_large",
    "allow_slug_reuse",
)

#: Closed enum of accepted ``source_type`` values. Safe to echo in error
#: messages (no NFR9 leakage risk — these are fixed strings, not paths).
ACCEPTED_SOURCE_TYPES: tuple[str, ...] = ("text", "markdown", "pdf", "url")


# ─── Error-code catalogue (frozen by 13-7, consumed by SkillResult) ───

#: Required parameter missing, or ``source_type`` outside the accepted
#: enum. Never echoes parameter VALUES (NFR9: ``source_ref`` can be a
#: filesystem path); only parameter names and the closed ``source_type``
#: enum are echoed back.
LIBRARY_INGEST_INVALID_ARGS = "LIBRARY_INGEST_INVALID_ARGS"

#: Plan-state refusal (FR37 / ADR-038). Library exists and is readable
#: but the billing plan does not allow new writes.
LIBRARY_READ_ONLY = "LIBRARY_READ_ONLY"

#: The Library is disabled — the schema file is missing at boot (NFR24)
#: or the sandbox could not be constructed. 13-20's boot warning covers
#: the operator-facing framing; this code covers the per-invocation
#: refusal.
LIBRARY_DISABLED = "LIBRARY_DISABLED"

#: Scaffold sentinel — argument validation passed but the pipeline
#: does not yet cover the requested source type. After 13-8 this is
#: still produced for ``source_type in {"pdf", "url"}`` (until 13-9
#: wires pdf); once a branch ships for a source type the pipeline
#: raises :data:`LIBRARY_INGEST_UNSUPPORTED_TYPE` instead of this code
#: for that type. Kept in the catalogue for historical parity.
LIBRARY_INGEST_NOT_IMPLEMENTED = "LIBRARY_INGEST_NOT_IMPLEMENTED"

#: ``source_type`` passed 13-7's closed-enum check but the current
#: ``_ingest_pipeline`` does not implement it (e.g. ``"pdf"`` before
#: 13-9 ships, ``"url"`` until a later story). Names the rejected
#: value because the enum is closed — safe to echo (NFR9).
LIBRARY_INGEST_UNSUPPORTED_TYPE = "LIBRARY_INGEST_UNSUPPORTED_TYPE"

#: ``source_ref`` resolved to bytes that contain a NUL byte or that
#: fail UTF-8 decode. Emitted with a generic "binary content" message
#: that never echoes the rejected filesystem path (NFR9).
LIBRARY_INGEST_BINARY_REJECTED = "LIBRARY_INGEST_BINARY_REJECTED"

#: ``source_ref`` could not be resolved — either the file does not
#: exist inside the sandbox or the path attempted to escape the root
#: (``../etc/passwd``). The error message never echoes the rejected
#: path (NFR9). ``source_content`` bypasses this code entirely.
LIBRARY_INGEST_SOURCE_NOT_FOUND = "LIBRARY_INGEST_SOURCE_NOT_FOUND"

#: Module-level summarizer seam is unset (``get_summarizer() is None``).
#: Default state at import time so unit tests can run without any real
#: LLM wiring; ``agent-claude`` calls :func:`set_summarizer` at boot
#: with the real Claude-backed summarizer.
LIBRARY_INGEST_NO_SUMMARIZER = "LIBRARY_INGEST_NO_SUMMARIZER"

#: The injected summarizer raised an unexpected exception (network
#: failure, model overload, invalid LLM output shape). Causes the
#: transaction to roll back via the context manager; the original
#: exception's ``str()`` is NOT propagated to the error_message
#: because it may embed upstream API payloads.
LIBRARY_INGEST_SUMMARIZER_FAILED = "LIBRARY_INGEST_SUMMARIZER_FAILED"

#: Source text exceeds the FR11 50K-word hard cap before the summarizer
#: is called. Added by story 13-10. The error message names the word
#: count, the cap, and a rough token-cost estimate (``word_count * 1.3``
#: input tokens plus 500 output tokens) so the caller knows what they
#: would have spent; it never echoes any filesystem path (NFR9). A
#: caller can override by passing ``parameters["accept_large"] = "true"``.
LIBRARY_INGEST_SOURCE_TOO_LARGE = "LIBRARY_INGEST_SOURCE_TOO_LARGE"

#: Writing the summarizer's drafted new pages would push the Library
#: past the FR12 500-page hard cap. Added by story 13-10. Raised from
#: inside the ``sandbox.transaction(...)`` block so the context manager
#: rolls back cleanly — no partial commit reaches git. The error
#: message echoes only the current page count and the cap (both
#: integers, NFR9-safe).
LIBRARY_INGEST_LIBRARY_FULL = "LIBRARY_INGEST_LIBRARY_FULL"

#: Story 13-11 (FR13) — post-ingest consistency gate: one or more of a
#: draft's ``cross_refs`` entries does not resolve to any existing
#: Library page and is not another draft in the same batch. Raised
#: from inside the ``sandbox.transaction(...)`` block so the 13-4
#: context manager rolls back cleanly — no partial / inconsistent
#: commit reaches git. The error message lists the offending
#: ``page → missing-slug`` pairs; those strings are the drafter's OWN
#: output, not sandbox-resolved filesystem paths, so NFR9 is satisfied.
LIBRARY_INGEST_INCONSISTENT_CROSS_REFS = "LIBRARY_INGEST_INCONSISTENT_CROSS_REFS"

#: Story 13-11 (FR13) — two or more drafts in the same batch normalize
#: to the same sandbox-relative path. A deterministic duplicate is
#: almost always a summarizer bug; we reject the whole batch so the
#: caller can re-run rather than silently losing one of the two.
LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG = (
    "LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG"
)

#: Story 13-11 (FR13) — a draft's slug is not filesystem-safe: empty
#: after normalization, missing the ``.md`` suffix, contains ``..`` /
#: ``.`` path components, contains an empty path component (``//``),
#: or contains a NUL byte. Rejecting these upstream of ``sandbox.write``
#: means ``PathEscapeError`` never fires and the ingest fails with a
#: clear, FR13-branded error code.
LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG = (
    "LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG"
)

#: Story 13-11 (FR13) — a draft's normalized path collides with an
#: existing Library page and the caller did NOT pass
#: ``parameters["allow_slug_reuse"] = "true"``. Until story 13-12
#: (source dedup) ships, every collision is treated as a hallucinated
#: re-summary rather than an intentional update. The error message
#: names the colliding slug and mentions the escape hatch.
LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION = (
    "LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION"
)

#: Story 13-13 (FR40) — a rendered page is missing one or more of the
#: required provenance front-matter fields (``source`` or
#: ``ingest_date``). The ingest writer is the sole producer of these
#: fields so in practice this code only fires on a regression or a
#: fault-injection test; the gate exists to enforce the invariant
#: symmetrically with the other consistency checks. The error message
#: names the offending slug (summarizer output — NOT a filesystem path,
#: NFR9-clean) and the missing field names.
LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE = (
    "LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE"
)

#: The supplied PDF bytes could not be parsed by the PDF library —
#: missing ``%PDF-`` magic, a ``.png`` renamed to ``.pdf`` (adversarial),
#: password-protected / encrypted PDFs, or structurally corrupt files.
#: Added by story 13-9. The error message NEVER propagates the upstream
#: pypdf exception string (NFR9 — may embed paths or raw bytes).
LIBRARY_INGEST_PDF_INVALID = "LIBRARY_INGEST_PDF_INVALID"

#: The supplied PDF parsed successfully but produced no usable text —
#: every page's ``extract_text()`` returned empty / whitespace-only.
#: Classic scanned-image PDFs land here. NFR26 pins the error MESSAGE
#: verbatim (see :data:`_PDF_EMPTY_EXTRACTION_MESSAGE`) — OCR is an
#: explicit non-goal in v1. Added by story 13-9.
LIBRARY_INGEST_PDF_EMPTY_EXTRACTION = "LIBRARY_INGEST_PDF_EMPTY_EXTRACTION"


# ─── Fixed message strings (pinned by tests) ──────────────────────────

# FR37 / ADR-038 — verbatim cloud-visible refusal when the user's plan is
# read-only. The string is intentionally mirrored in wh-cloud billing copy.
_READ_ONLY_MESSAGE = (
    "Your Library is in read-only mode. Upgrade to Pro to add new sources."
)

# NFR24 framing parity with 13-20's boot warning. Deliberately does NOT
# embed the Library root path — the boot path logs it once; duplicating it
# on every refusal would bloat cloud logs.
_DISABLED_MESSAGE = (
    "Library is disabled — schema file missing at boot (NFR24)."
)

# Legacy scaffold sentinel. 13-8 replaces the pipeline body for text /
# markdown but still mentions 13-9 (PDF) so downstream consumers can
# link unsupported-PDF refusals back to the relevant ticket.
_NOT_IMPLEMENTED_MESSAGE = (
    "library_ingest scaffolding only — content pipeline arrives in "
    "stories 13-8 (text/markdown) and 13-9 (PDF)."
)

# New error copy — deliberately terse. NFR9 forbids echoing any path
# or raw source bytes; these strings never interpolate user input.

_UNSUPPORTED_TYPE_FMT = (
    "library_ingest: source_type={value!r} is accepted by the skill "
    "schema but not yet implemented by the ingest pipeline "
    "(PDF arrives in story 13-9)."
)

_BINARY_REJECTED_MESSAGE = (
    "library_ingest: source contains binary content (NUL bytes or "
    "non-UTF-8 sequences) — only text and markdown are accepted."
)

_SOURCE_NOT_FOUND_MESSAGE = (
    "library_ingest: source_ref could not be resolved inside the "
    "Library sandbox."
)

_NO_SUMMARIZER_MESSAGE = (
    "library_ingest: no summarizer wired — the agent booted without "
    "an LLM summarizer callable. See wheelhouse.skills.library_ingest"
    ".set_summarizer()."
)

_SUMMARIZER_FAILED_MESSAGE = (
    "library_ingest: the summarizer raised an unexpected error; no "
    "pages were written and the transaction was rolled back."
)

# NFR26 user-facing wording pinned by epics-library FW-3.3. Tests match
# this byte-for-byte (em-dash included). Do NOT parameterize or add a
# path to this string — NFR9 forbids path echoing in error messages.
_PDF_EMPTY_EXTRACTION_MESSAGE = (
    "No extractable text found — OCR not supported in v1."
)

# Terse, NFR9-clean framing. Deliberately does NOT echo the upstream
# pypdf exception string — pypdf errors sometimes embed the path of the
# file being parsed, which would leak to cloud logs.
_PDF_INVALID_MESSAGE = (
    "library_ingest: source_type=pdf but the supplied content is not "
    "a valid PDF (magic number mismatch, encrypted, or corrupt)."
)


# ─── FR11 / FR12 limit-enforcement constants (Story 13-10) ────────────

#: FR11 hard cap on source word count. A source over this is rejected
#: with :data:`LIBRARY_INGEST_SOURCE_TOO_LARGE` before the summarizer is
#: called. Callers can override by passing
#: ``parameters["accept_large"] = "true"``.
MAX_INGEST_WORDS = 50_000

#: FR12 soft cap on Library page count. Ingesting into a Library at or
#: above this threshold logs a ``logger.warning`` but still proceeds.
PAGE_COUNT_WARN = 200

#: FR12 hard cap on Library page count. An ingest whose summarizer-drafted
#: NEW pages would push the total past this count is rejected with
#: :data:`LIBRARY_INGEST_LIBRARY_FULL`. Updates to existing pages are
#: always allowed — they don't grow the Library.
PAGE_COUNT_BLOCK = 500

#: Rough tokens-per-word ratio for the FR11 cost estimate. Deliberately
#: coarse — this is a warning copy aid, not a billing meter.
_TOKENS_PER_WORD_HEURISTIC = 1.3

#: Fixed output-token cost per summary invocation for the FR11 estimate.
_SUMMARY_OUTPUT_TOKENS = 500

# FR11 user-facing copy. Echoes only integers — no filesystem paths, no
# raw source text. Formatted with named keys so tests can assert on the
# numbers regardless of positional drift.
_SOURCE_TOO_LARGE_FMT = (
    "library_ingest: source has {word_count} words, exceeds "
    "{max} word cap (rough token estimate: ~{tokens} tokens). "
    "Pass accept_large=true to override."
)

# FR12 hard-block user-facing copy. Echoes only integers.
_LIBRARY_FULL_FMT = (
    "library_ingest: Library has {current} pages; adding {new} "
    "more would exceed the {cap}-page hard cap (FR12)."
)

# FR12 advisory warning — logger.warning ONLY, never returned in a
# SkillResult error_message. The "approaching 500-page soft cap"
# phrasing is canonical for test assertions.
_PAGE_COUNT_WARN_FMT = (
    "library_ingest: Library approaching 500-page soft cap ({current}/500)"
)

# FR11 override-active warning, logged when accept_large=true is honoured.
_ACCEPT_LARGE_WARN_FMT = (
    "library_ingest: accept_large=true override — ingesting "
    "{word_count}-word source (rough token estimate: ~{tokens} tokens)"
)

# ─── FR13 consistency-gate user-facing copy (Story 13-11) ─────────────

# All four messages echo ONLY slug strings that originated from the
# summarizer's draft output (or in the collision case, a slug the caller
# provided knowing it already exists). None of them ever interpolate a
# sandbox-resolved filesystem path — NFR9 stays clean.

_INCONSISTENT_CROSS_REFS_FMT = (
    "library_ingest: post-ingest consistency check failed — "
    "{count} dangling cross-reference(s): {pairs}"
)

_INCONSISTENT_CROSS_REFS_PAIR_LIMIT = 10

_INCONSISTENT_DUPLICATE_SLUG_FMT = (
    "library_ingest: post-ingest consistency check failed — "
    "two or more drafts share the same slug: {slug!r}"
)

_INCONSISTENT_UNSAFE_SLUG_FMT = (
    "library_ingest: post-ingest consistency check failed — "
    "unsafe draft slug {slug!r} ({reason})"
)

_INCONSISTENT_SLUG_COLLISION_FMT = (
    "library_ingest: post-ingest consistency check failed — "
    "draft slug {slug!r} collides with an existing Library page. "
    "Pass allow_slug_reuse=true to intentionally overwrite."
)

# Story 13-13 — NFR9-clean: ``slug`` is the summarizer's output string,
# not a filesystem-resolved path. ``missing`` is a pre-joined list of the
# missing provenance field names (``source``, ``ingest_date``).
_INCONSISTENT_MISSING_PROVENANCE_FMT = (
    "library_ingest: post-ingest consistency check failed — "
    "rendered page {slug!r} is missing required provenance "
    "field(s): {missing}"
)

# Story 13-13 — sidecar path at the Library sandbox root. Leading dot
# keeps it out of default index.md listings; ``_find_existing_by_source``
# already filters on the ``.md`` extension so it ignores the sidecar
# naturally. Tests may import this constant via the module path.
_PROVENANCE_PATH = ".provenance.json"

# Story 13-13 — forward-extensible sidecar schema version. Old readers
# that only understand version=1 will coexist with future v2 additions
# because ``_load_provenance`` tolerates anything it does not recognize.
_PROVENANCE_SCHEMA_VERSION = 1


_ACCEPT_LARGE_TRUTHY = frozenset({"true", "1", "yes", "y", "on"})


def _count_words(text: str) -> int:
    """Return the rough word count of ``text`` via ``str.split()``.

    Intentionally simple — collapses all whitespace runs, no
    language-aware tokenization. This is a runaway guard, not a billing
    meter (FR11 tolerates a ~20% fudge factor here).
    """
    return len(text.split())


def _estimate_tokens(word_count: int) -> int:
    """Rough LLM-token estimate for a ``word_count``-word source.

    Formula: ``word_count * 1.3`` input tokens + 500 output tokens per
    summary. Surfaced in FR11 error / warning copy so the caller can
    see what they would have spent.
    """
    return int(word_count * _TOKENS_PER_WORD_HEURISTIC) + _SUMMARY_OUTPUT_TOKENS


def _is_allow_slug_reuse(parameters: Mapping[str, Any]) -> bool:
    """Return ``True`` when ``parameters["allow_slug_reuse"]`` is truthy.

    Mirrors :func:`_is_accept_large` so the two string-typed overrides
    share the same case-insensitive truthy set
    (:data:`_ACCEPT_LARGE_TRUTHY`). Missing / empty / anything else is
    False. Added for story 13-11 so a caller can intentionally overwrite
    an existing Library page instead of having the consistency gate
    flag the collision.
    """
    raw = parameters.get("allow_slug_reuse")
    if raw is None:
        return False
    if isinstance(raw, bool):
        return raw
    return str(raw).strip().lower() in _ACCEPT_LARGE_TRUTHY


def _is_accept_large(parameters: Mapping[str, Any]) -> bool:
    """Return ``True`` when ``parameters["accept_large"]`` is truthy.

    The SkillInvocation wire format is string-typed
    (``map<string, string>``) so the override is compared case-insensitively
    against a fixed truthy set. Missing / empty / anything else is False.
    """
    raw = parameters.get("accept_large")
    if raw is None:
        return False
    if isinstance(raw, bool):
        return raw
    return str(raw).strip().lower() in _ACCEPT_LARGE_TRUTHY


def _count_library_pages(sandbox: LibrarySandbox) -> int:
    """Return the current count of content pages in the Library.

    Content pages are ``.md`` files returned by ``sandbox.list(".")``,
    excluding ``index.md`` (bookkeeping) and anything under ``.git/``
    (belt-and-braces — the sandbox shouldn't surface git internals but
    we don't rely on it). Used by the FR12 warn / block gates in
    :func:`_summarize_and_write`.
    """
    try:
        raw = sandbox.list(".")
    except Exception:  # noqa: BLE001 — caller stays robust on mock / missing dir
        return 0
    count = 0
    for path in raw:
        if not path.endswith(".md"):
            continue
        if path.startswith(".git/"):
            continue
        if path == "index.md":
            continue
        count += 1
    return count


def _is_unsafe_slug(normalized: str) -> tuple[bool, str]:
    """Return ``(is_unsafe, reason)`` for a normalized draft slug.

    Called by :func:`_check_ingest_consistency` on every draft path
    AFTER :func:`_normalize_page_path` has stripped leading slashes and
    flipped backslashes. The rules intentionally reject anything that
    would ever need a second thought — if a summarizer truly needs a
    weird slug, the fix is a better summarizer, not a softer gate.
    """
    if normalized == "":
        return True, "empty"
    if "\x00" in normalized:
        return True, "NUL byte"
    if normalized.startswith("/"):
        # _normalize_page_path should have stripped this — belt-and-braces.
        return True, "absolute path"
    if "//" in normalized:
        return True, "empty path component"
    if not normalized.endswith(".md"):
        return True, "not .md suffixed"
    # Per-component check: reject "." / ".." anywhere in the path.
    for part in normalized.split("/"):
        if part in (".", ".."):
            return True, "path-escape component"
        if part == "":
            # Redundant with the "//" check but catches a trailing slash.
            return True, "empty path component"
    return False, ""


def _check_ingest_consistency(
    sandbox: LibrarySandbox,
    drafts: list["PageDraft"],
    *,
    allow_slug_reuse: bool,
    rendered_pages: dict[str, str] | None = None,
) -> None:
    """Story 13-11 FR13 — post-summarizer / pre-write consistency gate.

    Runs four checks against the summarizer's draft batch:

    1. **Unsafe slugs** — any draft whose normalized path fails
       :func:`_is_unsafe_slug` raises
       :data:`LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG`.
    2. **Duplicate slugs** — two drafts whose normalized paths collide
       within the batch raise
       :data:`LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG`.
    3. **Slug collision with existing page** — a draft whose normalized
       path already exists in the Library raises
       :data:`LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION`, unless
       ``allow_slug_reuse`` is True (the caller intentionally wants to
       overwrite).
    4. **Dangling cross-refs** — every ``draft.cross_refs`` entry must
       resolve to either another draft in the batch OR an existing
       Library page. Any unresolved refs raise
       :data:`LIBRARY_INGEST_INCONSISTENT_CROSS_REFS` listing the
       offending ``page → missing-slug`` pairs.

    The checks run in the order above so that e.g. a ``..``-escaping
    slug fails with UNSAFE_SLUG rather than ricocheting into DUPLICATE
    or COLLISION. Cross-ref validation runs LAST because it needs the
    full "after-state" set of valid slugs.

    Raises:
        :class:`LibrarySkillError` with the appropriate
        ``LIBRARY_INGEST_INCONSISTENT_*`` code. Returns ``None`` on
        success.
    """
    # Step 1 — unsafe-slug screen (fail fast, per-draft).
    normalized_paths: list[str] = []
    for draft in drafts:
        normalized = _normalize_page_path(draft.path)
        unsafe, reason = _is_unsafe_slug(normalized)
        if unsafe:
            raise LibrarySkillError(
                _INCONSISTENT_UNSAFE_SLUG_FMT.format(
                    slug=draft.path, reason=reason
                ),
                code=LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG,
            )
        normalized_paths.append(normalized)

    # Step 2 — duplicate-slug screen within the batch.
    seen: set[str] = set()
    for normalized in normalized_paths:
        if normalized in seen:
            raise LibrarySkillError(
                _INCONSISTENT_DUPLICATE_SLUG_FMT.format(slug=normalized),
                code=LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG,
            )
        seen.add(normalized)

    # Step 3 — slug collision with existing pages (unless overridden).
    if not allow_slug_reuse:
        for normalized in normalized_paths:
            if normalized == "index.md":
                # index.md is bookkeeping — _update_index is the sole
                # writer and always performs a merge, never a clobber.
                continue
            try:
                exists = sandbox.exists(normalized)
            except Exception:  # noqa: BLE001 — robust against mock gaps
                exists = False
            if exists:
                raise LibrarySkillError(
                    _INCONSISTENT_SLUG_COLLISION_FMT.format(slug=normalized),
                    code=LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION,
                )

    # Step 4 — cross-reference dangle detection.
    # Build the "after-state" set: existing Library pages (via the
    # shared list helper) plus every draft path from this batch. A
    # cross_ref resolves if its normalized form is in the set.
    existing_pages = set(_list_existing_pages(sandbox))
    after_state = existing_pages | set(normalized_paths)
    # index.md is always a valid link target.
    after_state.add("index.md")

    dangling: list[tuple[str, str]] = []
    for draft in drafts:
        for ref in draft.cross_refs:
            normalized_ref = _normalize_page_path(ref)
            if normalized_ref in after_state:
                continue
            dangling.append((draft.path, ref))

    if dangling:
        # Clip at _INCONSISTENT_CROSS_REFS_PAIR_LIMIT so a 200-draft
        # hallucination storm doesn't blow up the error message.
        shown = dangling[:_INCONSISTENT_CROSS_REFS_PAIR_LIMIT]
        rendered = ", ".join(
            f"{page} -> {missing}" for page, missing in shown
        )
        clipped = len(dangling) - len(shown)
        if clipped > 0:
            rendered = f"{rendered} (and {clipped} more)"
        raise LibrarySkillError(
            _INCONSISTENT_CROSS_REFS_FMT.format(
                count=len(dangling), pairs=rendered
            ),
            code=LIBRARY_INGEST_INCONSISTENT_CROSS_REFS,
        )

    # Step 5 — FR40 provenance presence (Story 13-13). Every rendered
    # page MUST carry non-empty ``source`` AND ``ingest_date`` front-
    # matter fields. The ingest writer is the sole producer of these
    # fields, so in practice this check is a regression / fault-
    # injection guard — but the invariant is enforced symmetrically with
    # the other consistency checks so any future re-renderer cannot
    # silently ship pages with missing provenance.
    #
    # ``rendered_pages`` is None for legacy callers (unit tests that
    # exercise the gate without synthesizing rendered content); the
    # production call site in :func:`_summarize_and_write` always
    # passes the dict of ``{normalized_path: rendered_content}`` it is
    # about to stage via ``sandbox.write``.
    if rendered_pages:
        for slug, content in sorted(rendered_pages.items()):
            front_matter, _body = _parse_front_matter_local(content)
            missing: list[str] = []
            source_val = front_matter.get("source")
            if not isinstance(source_val, str) or not source_val:
                missing.append("source")
            ingest_val = front_matter.get("ingest_date")
            if not isinstance(ingest_val, str) or not ingest_val:
                missing.append("ingest_date")
            if missing:
                raise LibrarySkillError(
                    _INCONSISTENT_MISSING_PROVENANCE_FMT.format(
                        slug=slug, missing=", ".join(missing)
                    ),
                    code=LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE,
                )


# ─── Summarizer seam (13-8) ───────────────────────────────────────────


@dataclasses.dataclass(frozen=True)
class PageDraft:
    """A single page returned by the summarizer.

    ``path`` is a sandbox-relative POSIX path (e.g.
    ``"clients/acme/profile.md"``). ``body`` is the page body WITHOUT
    the YAML front-matter — the ingest writer prepends the front-matter.
    ``cross_refs`` is a list of sandbox-relative page paths this draft
    points at; dangle validation is deferred to 13-11.
    """

    path: str
    title: str
    body: str
    cross_refs: list[str] = dataclasses.field(default_factory=list)


@dataclasses.dataclass(frozen=True)
class _ExistingSourcePage:
    """A page already present in the Library that matches a dedup source.

    Story 13-12 — source dedup. When ``_find_existing_by_source`` detects
    that the incoming ``source_name`` already shows up in one or more
    existing pages' front-matter ``source`` fields, each match is
    wrapped in one of these records and (when the installed summarizer
    accepts a fifth argument) passed into the summarizer as the
    ``existing_source_pages`` parameter. The summarizer can then merge
    the new content into the existing structure rather than hallucinating
    a parallel page tree.

    ``slug`` is the sandbox-relative path of the existing page. ``body``
    is the page body with the front-matter block stripped. ``cross_refs``
    is whatever list was parsed out of the page's front-matter (possibly
    empty).
    """

    slug: str
    body: str
    cross_refs: tuple[str, ...] = ()


@dataclasses.dataclass(frozen=True)
class SummarizerResult:
    """Structured return from a Library summarizer callable.

    ``drafts`` lists the pages to write (one commit per whole result).
    ``tokens_used`` is forwarded verbatim into
    :attr:`SkillResult.library_tokens` (13-27 piggyback). ``summary``
    is a single-line human string used for the git commit subject.
    """

    drafts: list[PageDraft]
    tokens_used: int
    summary: str


#: Signature for the injected summarizer callable.
#:
#: The nominal type is a 4-arg callable
#: ``(source_text, source_name, user_summary_hint, existing_pages)``
#: — ``existing_pages`` is the sorted list of sandbox-relative page
#: paths already present, used by the LLM to decide whether to merge
#: into an existing page or create a new one.
#:
#: Story 13-12 added an OPTIONAL fifth parameter
#: ``existing_source_pages`` — a ``list[_ExistingSourcePage]`` of the
#: pages whose front-matter ``source`` field matches the incoming
#: source (i.e. this is a re-ingest). The ingest pipeline inspects the
#: installed summarizer's real signature via :mod:`inspect` and passes
#: the fifth argument ONLY when the summarizer's signature accepts it,
#: so pre-13-12 four-argument summarizers continue to work unchanged.
#: Do NOT rely on this type alias to enforce the fifth-arg shape at
#: type-check time — the alias is deliberately kept at 4 args to match
#: the baseline contract.
Summarizer = Callable[
    [str, str, "str | None", "list[str]"],
    SummarizerResult,
]

_SUMMARIZER: Summarizer | None = None


def set_summarizer(fn: Summarizer | None) -> None:
    """Install (or clear) the Library summarizer callable.

    ``agent-claude`` calls this exactly once at boot with a wrapper
    that adapts the async ``ClaudeClient.complete(...)`` call to the
    sync :data:`Summarizer` signature. Unit tests install a fake and
    then pass ``None`` in a fixture teardown to restore the default.
    """
    global _SUMMARIZER
    _SUMMARIZER = fn


def get_summarizer() -> Summarizer | None:
    """Return the currently-installed summarizer, or ``None``."""
    return _SUMMARIZER


# ─── Invocation entrypoint ────────────────────────────────────────────


def run_library_ingest(
    sandbox: LibrarySandbox | None,
    parameters: Mapping[str, str],
    *,
    library_status: str,
    invocation_id: str,
    skill_name: str = SKILL_NAME,
) -> SkillResult:
    """Handle a ``library_ingest`` SkillInvocation.

    Execution pipeline (frozen by 13-7):

    1. Disabled gate — ``library_status == "disabled"`` or ``sandbox is
       None`` → ``SkillResult(error_code=LIBRARY_DISABLED)``.
    2. Required-params validation — any missing key in ``REQUIRED_PARAMS``
       → ``SkillResult(error_code=LIBRARY_INGEST_INVALID_ARGS)``, naming
       only the missing key(s) — never echoing a value (NFR9).
    3. ``source_type`` enum validation — not a member of
       ``ACCEPTED_SOURCE_TYPES`` → ``SkillResult(error_code=
       LIBRARY_INGEST_INVALID_ARGS)`` naming the rejected value and the
       accepted list. Safe to echo because the enum is closed.
    4. Read-only gate — ``library_status == "read-only"`` →
       ``SkillResult(error_code=LIBRARY_READ_ONLY)`` with the verbatim
       FR37 message.
    5. Delegate to :func:`_ingest_pipeline`. In 13-7 the delegate always
       raises ``LibrarySkillError(LIBRARY_INGEST_NOT_IMPLEMENTED)``;
       this function catches it and translates into ``SkillResult``.
       13-8 and 13-9 replace the delegate body.

    The invocation handler NEVER raises — every path produces a
    ``SkillResult`` so the dispatch layer can publish it directly. Any
    unexpected ``LibrarySkillError`` from ``_ingest_pipeline`` is
    caught and converted using its ``.code`` attribute.

    Args:
        sandbox: the boot-constructed ``LibrarySandbox``, or ``None``
            when the agent booted with the Library disabled.
        parameters: raw ``SkillInvocation.parameters`` mapping.
        library_status: one of ``"enabled" | "read-only" | "disabled"``
            — the plan-state string propagated from ``WH_LIBRARY_STATUS``
            by ``run_startup`` (story 13-20).
        invocation_id: echoed into the returned ``SkillResult``.
        skill_name: echoed into the returned ``SkillResult``; defaults
            to ``SKILL_NAME`` so callers may rely on the module constant.

    Returns:
        A ``SkillResult`` ready to publish. Never raises.
    """
    # Step 1 — disabled gate.
    if library_status == "disabled" or sandbox is None:
        return SkillResult(
            invocation_id=invocation_id,
            skill_name=skill_name,
            success=False,
            error_code=LIBRARY_DISABLED,
            error_message=_DISABLED_MESSAGE,
        )

    # Step 2 — required params. Echo only the NAMES, never the VALUES.
    missing = [k for k in REQUIRED_PARAMS if not parameters.get(k)]
    if missing:
        return SkillResult(
            invocation_id=invocation_id,
            skill_name=skill_name,
            success=False,
            error_code=LIBRARY_INGEST_INVALID_ARGS,
            error_message=(
                "library_ingest: missing required parameter(s): "
                + ", ".join(missing)
            ),
        )

    # Step 3 — source_type enum. Safe to echo because the enum is closed.
    source_type = parameters["source_type"]
    if source_type not in ACCEPTED_SOURCE_TYPES:
        return SkillResult(
            invocation_id=invocation_id,
            skill_name=skill_name,
            success=False,
            error_code=LIBRARY_INGEST_INVALID_ARGS,
            error_message=(
                f"library_ingest: unsupported source_type={source_type!r}; "
                f"accepted: {ACCEPTED_SOURCE_TYPES}"
            ),
        )

    # Step 4 — read-only gate (FR37 / ADR-038, verbatim).
    if library_status == "read-only":
        return SkillResult(
            invocation_id=invocation_id,
            skill_name=skill_name,
            success=False,
            error_code=LIBRARY_READ_ONLY,
            error_message=_READ_ONLY_MESSAGE,
        )

    # Step 5 — delegate to the content pipeline. 13-8 implements the
    # text/markdown branch; 13-9 will add the PDF branch. The pipeline
    # returns a SkillResult with invocation_id left blank — this layer
    # fills it in so callers can rely on it.
    try:
        result = _ingest_pipeline(sandbox, parameters)
    except LibrarySkillError as exc:
        return SkillResult(
            invocation_id=invocation_id,
            skill_name=skill_name,
            success=False,
            error_code=exc.code or LIBRARY_INGEST_NOT_IMPLEMENTED,
            error_message=str(exc),
        )
    result.invocation_id = invocation_id
    result.skill_name = skill_name
    return result


def _ingest_pipeline(
    sandbox: LibrarySandbox,
    parameters: Mapping[str, Any],
) -> SkillResult:
    """Text / markdown content pipeline (Story 13-8).

    Contract assumed from ``run_library_ingest``: required keys are
    present and ``source_type`` is a member of
    :data:`ACCEPTED_SOURCE_TYPES`. The pipeline then:

    1. Rejects PDF / URL with :data:`LIBRARY_INGEST_UNSUPPORTED_TYPE`
       (13-9 replaces the branch for pdf).
    2. Resolves ``source_ref`` to UTF-8 text — see
       :func:`_resolve_source_text` for the three-tier resolution
       order and its NFR9-clean error surface.
    3. Looks up the module-level summarizer seam; missing → raise
       :data:`LIBRARY_INGEST_NO_SUMMARIZER` BEFORE opening any
       transaction (AC-9).
    4. Calls the summarizer with the source text, source basename,
       optional user hint, and the current list of sandbox pages.
       A raised exception becomes
       :data:`LIBRARY_INGEST_SUMMARIZER_FAILED` and the transaction
       is rolled back by the context manager (AC-10).
    5. Opens ``sandbox.transaction("ingest", <summary>)``, writes
       every draft with YAML front-matter, updates ``index.md`` under
       the ``## Ingested sources`` section, and populates
       ``commit_metadata`` (``sources``, ``pages_created``,
       ``pages_updated``, ``cross_references_added``).
    6. Returns :class:`SkillResult` with 13-27 piggyback fields set.
    """
    source_type = parameters["source_type"]
    if source_type not in ("text", "markdown", "pdf"):
        # ``url`` reaches here — never implemented in v1. Echoing the
        # value is safe because the enum is closed (no NFR9 leakage).
        raise LibrarySkillError(
            _UNSUPPORTED_TYPE_FMT.format(value=source_type),
            code=LIBRARY_INGEST_UNSUPPORTED_TYPE,
        )

    # Step 2 — source resolution. NFR9: no path in the error message.
    # The PDF branch (13-9) extracts text via pypdf and collects a
    # partial-extraction flag for the commit metadata; the text /
    # markdown branch (13-8) goes through the original UTF-8 resolver.
    extra_metadata: dict[str, Any] = {}
    if source_type == "pdf":
        source_text, source_name, partial = _resolve_pdf_text(
            sandbox, parameters
        )
        if partial:
            extra_metadata["pdf_partial_extraction"] = True
    else:
        source_text, source_name = _resolve_source_text(sandbox, parameters)

    # Steps 3–6 — shared with the text/markdown path. ``accept_large``
    # (Story 13-10 FR11 override) is resolved once here and forwarded
    # so the pre-summarizer size gate knows whether to proceed.
    return _summarize_and_write(
        sandbox,
        source_text=source_text,
        source_name=source_name,
        source_type=source_type,
        user_hint_raw=parameters.get("user_summary_hint"),
        extra_metadata=extra_metadata,
        accept_large=_is_accept_large(parameters),
        allow_slug_reuse=_is_allow_slug_reuse(parameters),
    )


def _summarize_and_write(
    sandbox: LibrarySandbox,
    *,
    source_text: str,
    source_name: str,
    source_type: str,
    user_hint_raw: Any,
    extra_metadata: Mapping[str, Any],
    accept_large: bool = False,
    allow_slug_reuse: bool = False,
) -> SkillResult:
    """Summarizer call + single-transaction write path (shared).

    Extracted from :func:`_ingest_pipeline` by story 13-9 so the PDF
    branch reuses the exact same summarizer → writer → commit_metadata
    code as the text/markdown branch. ``extra_metadata`` lets a caller
    thread branch-specific keys (e.g. ``pdf_partial_extraction``) into
    the transaction's ``commit_metadata`` without this helper needing
    to know anything about the upstream source format.
    """
    # ── FR11 pre-summarizer size gate (Story 13-10) ──────────────────
    # Count words BEFORE the summarizer is called so a runaway source
    # never spends any LLM tokens. ``accept_large=true`` overrides with
    # a warning so power users can still push a one-off mega-source
    # through without editing the constant.
    word_count = _count_words(source_text)
    if word_count > MAX_INGEST_WORDS:
        token_estimate = _estimate_tokens(word_count)
        if accept_large:
            # Override path — proceed with a warning. (Kept as a seam
            # in case future stories want to gate the override behind
            # plan state.)
            logger.warning(
                _ACCEPT_LARGE_WARN_FMT.format(
                    word_count=word_count, tokens=token_estimate
                )
            )
        else:
            raise LibrarySkillError(
                _SOURCE_TOO_LARGE_FMT.format(
                    word_count=word_count,
                    max=MAX_INGEST_WORDS,
                    tokens=token_estimate,
                ),
                code=LIBRARY_INGEST_SOURCE_TOO_LARGE,
            )

    # Step 3 — summarizer wired? Do this BEFORE opening a transaction
    # so the "sandbox.transaction is never called" invariant holds.
    summarizer = get_summarizer()
    if summarizer is None:
        raise LibrarySkillError(
            _NO_SUMMARIZER_MESSAGE,
            code=LIBRARY_INGEST_NO_SUMMARIZER,
        )

    # ── FR12 pre-transaction advisory warning (Story 13-10) ──────────
    # Emitted BEFORE the transaction opens so the page count reflects
    # the on-disk state, not the mid-transaction staging area. Log-only
    # (not in SkillResult) — 13-27 didn't add a warnings channel.
    existing_page_count = _count_library_pages(sandbox)
    if existing_page_count >= PAGE_COUNT_WARN:
        logger.warning(
            _PAGE_COUNT_WARN_FMT.format(current=existing_page_count)
        )

    user_hint = str(user_hint_raw) if user_hint_raw else None
    existing_pages = _list_existing_pages(sandbox)

    # ── FR16 source dedup on re-ingest (Story 13-12) ─────────────────
    # Scan every existing page's front-matter for a ``source`` field
    # matching ``source_name``. If any match, we:
    #
    #   1. Auto-flip ``allow_slug_reuse`` to True so the 13-11
    #      consistency gate treats same-slug drafts as intentional
    #      updates rather than hallucinated re-summaries.
    #   2. Hand the matched pages (slug + body + cross_refs) to the
    #      summarizer via the optional 5th parameter — provided the
    #      summarizer's signature accepts it. 4-arg summarizers keep
    #      working unchanged (AC-7).
    #   3. Tag the transaction operation as ``"re-ingest"`` and set
    #      ``commit_metadata["source_dedup"] = True`` so the 13-4
    #      commit history makes the update distinguishable from a
    #      fresh ingest (AC-9).
    dedup_matches = _find_existing_by_source(sandbox, source_name)
    source_dedup_active = bool(dedup_matches)
    if source_dedup_active:
        allow_slug_reuse = True
        logger.info(
            "library_ingest: source dedup matched %d existing page(s) "
            "for re-ingest of %s",
            len(dedup_matches),
            source_name,
        )

    # Step 4 — call the summarizer. We wrap ONLY the summarizer call
    # in the try so that a bug inside the writer still surfaces as a
    # real traceback instead of being masked behind SUMMARIZER_FAILED.
    try:
        if _summarizer_accepts_dedup_arg(summarizer):
            result = summarizer(
                source_text,
                source_name,
                user_hint,
                existing_pages,
                dedup_matches,  # type: ignore[call-arg]  # optional 5th arg
            )
        else:
            result = summarizer(
                source_text, source_name, user_hint, existing_pages
            )
    except LibrarySkillError:
        raise  # already coded; let run_library_ingest translate
    except BaseException as exc:  # noqa: BLE001 — deliberate LLM boundary
        logger.warning(
            "library_ingest summarizer raised %s", type(exc).__name__
        )
        raise LibrarySkillError(
            _SUMMARIZER_FAILED_MESSAGE,
            code=LIBRARY_INGEST_SUMMARIZER_FAILED,
        ) from exc

    # Step 5 — write pages + index entries inside a single transaction.
    commit_subject = result.summary or f"ingest {source_name}"
    pages_created: list[str] = []
    pages_updated: list[str] = []
    cross_refs_added = 0
    ingest_ts = _utc_now_iso()

    tx_operation = "re-ingest" if source_dedup_active else "ingest"
    with sandbox.transaction(tx_operation, commit_subject) as handle:
        # ── FR12 hard-block page-count gate (Story 13-10) ────────────
        # We re-count existing content pages inside the transaction
        # (using the same on-disk state as the pre-transaction warn
        # gate — LibrarySandbox.transaction does not stage writes in
        # a separate view) and compute how many of the summarizer's
        # drafts are NEW pages vs. updates to existing ones. If adding
        # the new ones would cross the hard cap, raise
        # LIBRARY_INGEST_LIBRARY_FULL — the context manager rolls back
        # the (still-empty) transaction and no writes are persisted.
        _existing_now = _count_library_pages(sandbox)
        _new_page_count = 0
        for _draft in result.drafts:
            _p = _normalize_page_path(_draft.path)
            if _p == "index.md":
                continue
            if not sandbox.exists(_p):
                _new_page_count += 1
        if _existing_now + _new_page_count > PAGE_COUNT_BLOCK:
            raise LibrarySkillError(
                _LIBRARY_FULL_FMT.format(
                    current=_existing_now,
                    new=_new_page_count,
                    cap=PAGE_COUNT_BLOCK,
                ),
                code=LIBRARY_INGEST_LIBRARY_FULL,
            )

        # ── FR13 post-ingest consistency gate (Story 13-11) ──────────
        # Verifies unsafe slugs, duplicate batch slugs, slug-collision
        # with existing pages, dangling cross-references, AND (Story
        # 13-13 FR40) that every rendered page carries the required
        # ``source`` / ``ingest_date`` provenance front-matter fields.
        # Raising here triggers the transaction context manager's
        # rollback path, so no partial / inconsistent commit reaches
        # git. Ordering: AFTER the LIBRARY_FULL hard block so a
        # 500-page Library fails with the more-specific LIBRARY_FULL
        # code even when the drafts also happen to have dangling refs.
        #
        # 13-13: pre-render every draft into the
        # ``{normalized_path: rendered_content}`` dict BEFORE the
        # consistency gate so the provenance check can inspect the
        # exact bytes we are about to stage via ``sandbox.write``.
        rendered_pages: dict[str, str] = {}
        for draft in result.drafts:
            page_path = _normalize_page_path(draft.path)
            rendered_pages[page_path] = _render_page(
                body=draft.body,
                source_name=source_name,
                source_type=source_type,
                ingest_ts=ingest_ts,
                cross_refs=list(draft.cross_refs),
            )

        _check_ingest_consistency(
            sandbox,
            list(result.drafts),
            allow_slug_reuse=allow_slug_reuse,
            rendered_pages=rendered_pages,
        )

        new_page_entries: list[tuple[str, str]] = []  # (path, title)
        written_slugs: list[str] = []
        for draft in result.drafts:
            page_path = _normalize_page_path(draft.path)
            is_update = sandbox.exists(page_path)
            content = rendered_pages[page_path]
            sandbox.write(page_path, content)
            written_slugs.append(page_path)
            cross_refs_added += len(draft.cross_refs)
            if is_update:
                pages_updated.append(page_path)
            else:
                pages_created.append(page_path)
                new_page_entries.append((page_path, draft.title))

        if new_page_entries:
            index_changed_as_update = _update_index(
                sandbox, new_page_entries
            )
            if index_changed_as_update:
                if "index.md" not in pages_updated:
                    pages_updated.append("index.md")
            else:
                if "index.md" not in pages_created:
                    pages_created.append("index.md")

        pages_created.sort()
        pages_updated.sort()
        handle.commit_metadata["sources"] = [source_name]
        handle.commit_metadata["pages_created"] = pages_created
        handle.commit_metadata["pages_updated"] = pages_updated
        handle.commit_metadata["cross_references_added"] = cross_refs_added
        if source_dedup_active:
            handle.commit_metadata["source_dedup"] = True
        for key, value in extra_metadata.items():
            handle.commit_metadata[key] = value

        # ── FR40 provenance sidecar write (Story 13-13) ──────────────
        # Load the existing ``.provenance.json`` (if any), fold in one
        # entry per written slug (both created and updated — every
        # write must refresh its history entry), and stage the updated
        # sidecar inside the SAME transaction as the page writes so
        # the commit atomically captures page + sidecar state. The
        # write is the LAST step inside the transaction — any
        # preceding failure rolls back via the context manager before
        # we touch the sidecar on disk.
        #
        # Ingester / commit-SHA fields are intentionally not stored
        # here; see the story's Context section for the rationale.
        provenance_existing = _load_provenance(sandbox)
        provenance_entries: list[tuple[str, str, str]] = [
            (slug, source_name, source_type) for slug in written_slugs
        ]
        provenance_updated = _update_provenance(
            provenance_existing, provenance_entries, ingest_ts
        )
        sandbox.write(
            _PROVENANCE_PATH, _serialize_provenance(provenance_updated)
        )

    # Step 6 — SkillResult. invocation_id / skill_name are filled in
    # by ``run_library_ingest`` from its own parameters; this function
    # is never called directly by the dispatch layer.
    drafted_page_count = len(result.drafts)
    output = (
        f"Ingested {source_name}: {len(pages_created)} new page(s), "
        f"{len(pages_updated)} updated, {cross_refs_added} cross-ref(s)."
    )
    return SkillResult(
        invocation_id="",  # run_library_ingest fills this in
        skill_name=SKILL_NAME,
        success=True,
        output=output,
        library_tokens=int(result.tokens_used),
        library_page_count=drafted_page_count,
        library_last_ingest_at=ingest_ts,
    )


# ─── Helpers (13-8) ───────────────────────────────────────────────────


def _utc_now_iso() -> str:
    """Return an ISO-8601 UTC timestamp to the second, with trailing ``Z``.

    Format pinned by AC-2: ``YYYY-MM-DDTHH:MM:SSZ``. Python's
    ``datetime.isoformat()`` emits ``+00:00`` instead of ``Z`` so we
    format explicitly. A single module-level seam is used so tests can
    monkeypatch it deterministically.
    """
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _resolve_source_text(
    sandbox: LibrarySandbox,
    parameters: Mapping[str, Any],
) -> tuple[str, str]:
    """Return ``(source_text, source_name)`` for the ingest request.

    Resolution order (AC-6 / AC-7):

    1. If ``parameters["source_content"]`` is present and non-empty,
       use it verbatim as the source text and derive the
       ``source_name`` from ``os.path.basename(source_ref)`` — the
       sandbox is NOT read. This is how agents pass inline blobs the
       user pasted into the conversation.
    2. Else try ``sandbox.read(source_ref)``. ``PathEscapeError`` (the
       ``../etc/passwd`` case), ``FileNotFoundError``, ``IsADirectoryError``
       and ``UnicodeDecodeError`` collapse into
       :data:`LIBRARY_INGEST_SOURCE_NOT_FOUND` /
       :data:`LIBRARY_INGEST_BINARY_REJECTED` with NO path echoed.

    Binary-content rejection (AC-5): after the raw bytes are decoded
    to ``str``, we additionally screen for NUL bytes. The sandbox's
    ``read`` already opens the file with ``encoding="utf-8"`` (which
    rejects undecodable sequences), so NUL is the only residual
    binary marker we need to check ourselves.
    """
    source_ref = str(parameters["source_ref"])
    source_name = os.path.basename(source_ref) or source_ref

    inline = parameters.get("source_content")
    if inline is not None and inline != "":
        if not isinstance(inline, str):
            raise LibrarySkillError(
                _BINARY_REJECTED_MESSAGE,
                code=LIBRARY_INGEST_BINARY_REJECTED,
            )
        if "\x00" in inline:
            raise LibrarySkillError(
                _BINARY_REJECTED_MESSAGE,
                code=LIBRARY_INGEST_BINARY_REJECTED,
            )
        return inline, source_name

    # Filesystem path — must stay inside the sandbox. NFR9: no path
    # ever leaks into the error message.
    try:
        text = sandbox.read(source_ref)
    except (PathEscapeError, FileNotFoundError, IsADirectoryError, NotADirectoryError, ValueError):
        raise LibrarySkillError(
            _SOURCE_NOT_FOUND_MESSAGE,
            code=LIBRARY_INGEST_SOURCE_NOT_FOUND,
        ) from None
    except UnicodeDecodeError:
        raise LibrarySkillError(
            _BINARY_REJECTED_MESSAGE,
            code=LIBRARY_INGEST_BINARY_REJECTED,
        ) from None

    if "\x00" in text:
        raise LibrarySkillError(
            _BINARY_REJECTED_MESSAGE,
            code=LIBRARY_INGEST_BINARY_REJECTED,
        )
    return text, source_name


_PDF_MAGIC = b"%PDF-"


def _resolve_pdf_bytes(
    sandbox: LibrarySandbox,
    parameters: Mapping[str, Any],
) -> tuple[bytes, str]:
    """Return ``(pdf_bytes, source_name)`` for a ``source_type="pdf"`` call.

    Resolution order (story 13-9):

    1. If ``parameters["source_content"]`` is present and non-empty,
       use it directly. Accepted shapes:

       - ``bytes`` — passed verbatim to pypdf.
       - ``str`` whose first bytes (after Latin-1 re-encoding) match
         ``%PDF-`` — treated as a raw bytestring smuggled through a
         string-typed proto field.
       - ``str`` that looks like base64 — decoded and then re-checked
         for the ``%PDF-`` magic.

       Any other string falls through to
       :data:`LIBRARY_INGEST_PDF_INVALID` with no path leakage.

    2. Else :meth:`LibrarySandbox.read_bytes` resolves ``source_ref``
       inside the sandbox, collapsing :class:`PathEscapeError` /
       ``FileNotFoundError`` / ``IsADirectoryError`` /
       ``NotADirectoryError`` / ``ValueError`` into
       :data:`LIBRARY_INGEST_SOURCE_NOT_FOUND` with NO path echoed
       (NFR9).

    The returned bytes are NOT yet validated by pypdf — only the magic
    number is checked. :func:`_extract_pdf_text` runs the full pypdf
    parse and may still raise :data:`LIBRARY_INGEST_PDF_INVALID` on
    structurally-corrupt or encrypted input.
    """
    import base64
    import binascii

    source_ref = str(parameters["source_ref"])
    source_name = os.path.basename(source_ref) or source_ref

    inline = parameters.get("source_content")
    if inline is not None and inline != "":
        if isinstance(inline, bytes):
            raw = inline
        elif isinstance(inline, str):
            # Fast path: the string IS the raw bytes (Latin-1 smuggling).
            try:
                candidate = inline.encode("latin-1")
            except UnicodeEncodeError:
                candidate = b""
            if candidate.startswith(_PDF_MAGIC):
                raw = candidate
            else:
                # Try base64 decode. A non-base64 string will raise
                # ``binascii.Error`` (or return gibberish that fails
                # the magic check below) — either way we reject as
                # LIBRARY_INGEST_PDF_INVALID.
                try:
                    raw = base64.b64decode(inline, validate=False)
                except (binascii.Error, ValueError):
                    raise LibrarySkillError(
                        _PDF_INVALID_MESSAGE,
                        code=LIBRARY_INGEST_PDF_INVALID,
                    ) from None
        else:
            raise LibrarySkillError(
                _PDF_INVALID_MESSAGE,
                code=LIBRARY_INGEST_PDF_INVALID,
            )

        if not raw.startswith(_PDF_MAGIC):
            raise LibrarySkillError(
                _PDF_INVALID_MESSAGE,
                code=LIBRARY_INGEST_PDF_INVALID,
            )
        return raw, source_name

    # Filesystem path — must stay inside the sandbox. NFR9: no path
    # ever leaks into the error message.
    try:
        raw = sandbox.read_bytes(source_ref)
    except (
        PathEscapeError,
        FileNotFoundError,
        IsADirectoryError,
        NotADirectoryError,
        ValueError,
    ):
        raise LibrarySkillError(
            _SOURCE_NOT_FOUND_MESSAGE,
            code=LIBRARY_INGEST_SOURCE_NOT_FOUND,
        ) from None

    if not raw.startswith(_PDF_MAGIC):
        raise LibrarySkillError(
            _PDF_INVALID_MESSAGE,
            code=LIBRARY_INGEST_PDF_INVALID,
        )
    return raw, source_name


def _extract_pdf_text(pdf_bytes: bytes) -> tuple[str, bool]:
    """Return ``(joined_text, partial_extraction_flag)``.

    Parses ``pdf_bytes`` with :mod:`pypdf`, calls ``extract_text()`` on
    every page individually, and joins the non-empty results with a
    ``\\n\\n`` separator. Per-page extraction errors are caught and
    logged at WARNING with only ``type(exc).__name__`` — NFR9 forbids
    embedding the exception ``str()`` because upstream libraries can
    leak paths.

    The empty-text gate (NFR26) is enforced here: if NO page produced
    any non-whitespace text, :data:`LIBRARY_INGEST_PDF_EMPTY_EXTRACTION`
    is raised with the verbatim user-facing message.

    Raises:
        :class:`LibrarySkillError` with code
        :data:`LIBRARY_INGEST_PDF_INVALID` when pypdf cannot open the
        file (encrypted, corrupt, unrecognized structure) or when the
        upstream library is missing.
        :class:`LibrarySkillError` with code
        :data:`LIBRARY_INGEST_PDF_EMPTY_EXTRACTION` when the parse
        succeeded but every page was empty (scanned images, NFR26).
    """
    try:
        import pypdf  # type: ignore[import-not-found]
    except ImportError as exc:  # pragma: no cover — dep pinned in pyproject
        logger.warning(
            "library_ingest: pypdf not installed (%s)", type(exc).__name__
        )
        raise LibrarySkillError(
            _PDF_INVALID_MESSAGE,
            code=LIBRARY_INGEST_PDF_INVALID,
        ) from exc

    import io

    try:
        reader = pypdf.PdfReader(io.BytesIO(pdf_bytes))
    except BaseException as exc:  # noqa: BLE001 — upstream raises a zoo
        logger.warning(
            "library_ingest: pypdf.PdfReader failed %s",
            type(exc).__name__,
        )
        raise LibrarySkillError(
            _PDF_INVALID_MESSAGE,
            code=LIBRARY_INGEST_PDF_INVALID,
        ) from exc

    # Encrypted PDFs surface as ``reader.is_encrypted`` — attempting
    # extract_text() on them raises a pypdf exception we'd then have
    # to map anyway. Fail fast here with the INVALID code; a future
    # story can add a dedicated "encrypted, please decrypt" code if
    # user feedback warrants it.
    if getattr(reader, "is_encrypted", False):
        logger.warning("library_ingest: encrypted PDF rejected")
        raise LibrarySkillError(
            _PDF_INVALID_MESSAGE,
            code=LIBRARY_INGEST_PDF_INVALID,
        )

    extracted_page_texts: list[str] = []
    skipped_pages = 0
    for page in reader.pages:
        try:
            text = page.extract_text() or ""
        except BaseException as exc:  # noqa: BLE001 — pypdf zoo
            skipped_pages += 1
            logger.warning(
                "library_ingest: PDF page extract_text() raised %s",
                type(exc).__name__,
            )
            continue
        if text.strip():
            extracted_page_texts.append(text)

    joined = "\n\n".join(extracted_page_texts)
    if not joined.strip():
        # Empty-text gate — NFR26. Every page was empty or skipped.
        raise LibrarySkillError(
            _PDF_EMPTY_EXTRACTION_MESSAGE,
            code=LIBRARY_INGEST_PDF_EMPTY_EXTRACTION,
        )

    partial = skipped_pages > 0
    return joined, partial


def _resolve_pdf_text(
    sandbox: LibrarySandbox,
    parameters: Mapping[str, Any],
) -> tuple[str, str, bool]:
    """Thin wrapper that chains :func:`_resolve_pdf_bytes` and
    :func:`_extract_pdf_text` and returns ``(text, source_name, partial)``.

    Kept as a seam so that a future story can swap the extraction
    backend (pdfplumber, tika, OCR) without touching the branching
    logic inside :func:`_ingest_pipeline`.
    """
    pdf_bytes, source_name = _resolve_pdf_bytes(sandbox, parameters)
    text, partial = _extract_pdf_text(pdf_bytes)
    return text, source_name, partial


def _list_existing_pages(sandbox: LibrarySandbox) -> list[str]:
    """Return sorted sandbox-relative markdown page paths.

    Filters out anything that is not ``.md`` and anything under
    ``.git/`` (belt-and-braces — git shouldn't be inside ``list`` but
    we don't rely on it). Returns an empty list if the sandbox is
    empty or ``list`` is not available (e.g. a mock that didn't
    stub it).
    """
    try:
        raw = sandbox.list(".")
    except Exception:  # noqa: BLE001 — caller must stay robust
        return []
    pages: list[str] = []
    for path in raw:
        if path.endswith(".md") and not path.startswith(".git/"):
            pages.append(path)
    pages.sort()
    return pages


_FRONT_MATTER_DELIMITER = "---"


def _parse_front_matter_local(text: str) -> tuple[dict[str, Any], str]:
    """Minimal subset-YAML front-matter parser (Story 13-12).

    Deliberately duplicated from ``library_lint._parse_front_matter``
    rather than imported — the ingest pipeline should not depend on the
    lint skill's implementation details. The subset is:

    * Opening line must be exactly ``---``.
    * Closing line must be exactly ``---`` somewhere later in the file.
    * Body scalars are parsed as ``key: value`` strings with optional
      quoted scalars (``"acme.md"`` → ``acme.md``).
    * Flow lists ``key: [a, b, c]`` are parsed into ``list[str]``.
    * Unterminated / missing front-matter collapses to ``({}, text)``
      — the helper never raises, which matters because the ingest
      pipeline calls it on every ``.md`` file in the sandbox and any
      crash would abort the whole ingest.

    Returns ``(front_matter_dict, body_without_front_matter)``.
    """
    if not text.startswith(_FRONT_MATTER_DELIMITER):
        return {}, text

    lines = text.splitlines(keepends=True)
    if not (lines and lines[0].rstrip("\r\n") == _FRONT_MATTER_DELIMITER):
        return {}, text

    close_idx = -1
    for i in range(1, len(lines)):
        if lines[i].rstrip("\r\n") == _FRONT_MATTER_DELIMITER:
            close_idx = i
            break
    if close_idx == -1:
        return {}, text

    fm_lines = lines[1:close_idx]
    body = "".join(lines[close_idx + 1:])
    if body.startswith("\n"):
        body = body[1:]

    front_matter: dict[str, Any] = {}
    for raw in fm_lines:
        line = raw.rstrip("\r\n")
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if ":" not in line:
            continue
        key, _, value = line.partition(":")
        key = key.strip()
        value = value.strip()
        if not key:
            continue
        front_matter[key] = _parse_scalar_or_list_local(value)
    return front_matter, body


def _parse_scalar_or_list_local(value: str) -> Any:
    """Parse ``"[a, b]"`` → ``["a","b"]``; otherwise return the scalar.

    Twin of ``library_lint._parse_scalar_or_list`` — see
    :func:`_parse_front_matter_local` for the duplication rationale.
    """
    if value.startswith("[") and value.endswith("]"):
        inner = value[1:-1].strip()
        if not inner:
            return []
        parts = [p.strip().strip("'").strip('"') for p in inner.split(",")]
        return [p for p in parts if p]
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        return value[1:-1]
    return value


def _find_existing_by_source(
    sandbox: LibrarySandbox,
    source_name: str,
) -> list[_ExistingSourcePage]:
    """Return every page whose front-matter ``source`` equals ``source_name``.

    Story 13-12 — the core dedup lookup. Iterates every ``.md`` file
    under the sandbox (excluding ``index.md`` and anything under
    ``.git/``), parses its front-matter with
    :func:`_parse_front_matter_local`, and emits one
    :class:`_ExistingSourcePage` record per match.

    Robustness guarantees (AC-10):

    * Pages that fail to read, fail to parse, or have no front-matter at
      all are silently skipped — the dedup lookup must NEVER crash the
      ingest pipeline.
    * Pages with front-matter but no ``source`` key are skipped.
    * ``source_name`` is compared byte-for-byte against the parsed
      ``source`` field. The caller is responsible for basename-ing the
      incoming ``source_ref`` — :func:`_resolve_source_text` already
      does this, matching how story 13-8 stores the field in the first
      place.
    """
    if not source_name:
        return []
    try:
        raw_list = sandbox.list(".")
    except Exception:  # noqa: BLE001 — robust against mock gaps
        return []

    matches: list[_ExistingSourcePage] = []
    for rel in raw_list:
        if not rel.endswith(".md"):
            continue
        if rel.startswith(".git/"):
            continue
        if rel == "index.md":
            continue
        try:
            raw = sandbox.read(rel)
        except Exception:  # noqa: BLE001 — missing / unreadable → skip
            continue
        try:
            front_matter, body = _parse_front_matter_local(raw)
        except Exception:  # noqa: BLE001 — malformed YAML → skip
            continue
        stored_source = front_matter.get("source")
        if stored_source != source_name:
            continue
        cross_refs_raw = front_matter.get("cross_refs", [])
        if isinstance(cross_refs_raw, str):
            cross_refs_raw = [cross_refs_raw]
        if not isinstance(cross_refs_raw, list):
            cross_refs_raw = []
        cross_refs = tuple(str(r) for r in cross_refs_raw if r)
        matches.append(
            _ExistingSourcePage(slug=rel, body=body, cross_refs=cross_refs)
        )
    # Sort for deterministic ordering across platforms.
    matches.sort(key=lambda m: m.slug)
    return matches


def _summarizer_accepts_dedup_arg(summarizer: Summarizer) -> bool:
    """Return ``True`` if ``summarizer`` accepts a 5th positional arg.

    Story 13-12 — source dedup. The summarizer contract was extended
    with an OPTIONAL ``existing_source_pages`` fifth parameter; pre-13-12
    summarizers still use the 4-arg signature. We introspect via
    :func:`inspect.signature` and count non-VAR parameters. A callable
    for which ``inspect.signature`` raises (C-implemented builtins,
    some ``functools.partial`` shapes) is conservatively treated as
    4-arg — we fall back to the safe call shape.
    """
    try:
        sig = inspect.signature(summarizer)
    except (TypeError, ValueError):
        return False
    count = 0
    for param in sig.parameters.values():
        if param.kind in (
            inspect.Parameter.VAR_POSITIONAL,
            inspect.Parameter.VAR_KEYWORD,
        ):
            # ``*args`` / ``**kwargs`` — be generous and allow the 5-arg
            # call path. A summarizer that accepts **kwargs clearly
            # opted into whatever the framework passes.
            return True
        count += 1
        if count >= 5:
            return True
    return False


def _normalize_page_path(path: str) -> str:
    """Normalize a draft path to a POSIX sandbox-relative string.

    We do NOT validate containment here — ``sandbox.write`` will raise
    :class:`PathEscapeError` for any ``..`` escape which then bubbles
    out of the transaction and rolls it back. This helper only
    handles separator normalization and stripping leading slashes so
    a drafter that returned ``/clients/acme/profile.md`` still lands
    in the right place.
    """
    normalized = path.replace("\\", "/").lstrip("/")
    return normalized


def _render_page(
    *,
    body: str,
    source_name: str,
    source_type: str,
    ingest_ts: str,
    cross_refs: list[str],
) -> str:
    """Render a page body with a minimal YAML front-matter block.

    Format pinned by FR40 / Story 13-13: four top-level keys in fixed
    order (``source``, ``source_type``, ``ingest_date``, ``cross_refs``),
    then a blank line, then the body. ``source`` and ``source_type`` are
    emitted as JSON strings so that any special characters (quotes,
    colons) round-trip cleanly without pulling in a YAML dependency.
    ``cross_refs`` is a YAML flow list with each item JSON-quoted. The
    rendered bytes are stable for the same inputs.

    Prior to 13-13 this function emitted three fields (no
    ``source_type``). 13-13 adds the field so downstream consumers
    (citations, status, future migrations) can render format-aware
    citations without re-inferring the type. Pre-13-13 pages remain
    parseable — ``_parse_front_matter_local`` handles unknown and
    missing keys gracefully.
    """
    if not body.endswith("\n"):
        body = body + "\n"
    refs_yaml = ", ".join(json.dumps(r) for r in cross_refs)
    front_matter = (
        "---\n"
        f"source: {json.dumps(source_name)}\n"
        f"source_type: {json.dumps(source_type)}\n"
        f"ingest_date: {ingest_ts}\n"
        f"cross_refs: [{refs_yaml}]\n"
        "---\n"
        "\n"
    )
    return front_matter + body


_INDEX_SECTION_HEADER = "## Ingested sources"


def _update_index(
    sandbox: LibrarySandbox,
    new_entries: list[tuple[str, str]],
) -> bool:
    """Append new page links to ``index.md`` under ``## Ingested sources``.

    Returns ``True`` if the index already existed (update), ``False``
    if it was created from scratch. Duplicate entries (same path) are
    not added twice. The pre-existing content of ``index.md`` is
    preserved verbatim; only the section body is extended.
    """
    existed = sandbox.exists("index.md")
    if existed:
        try:
            current = sandbox.read("index.md")
        except (FileNotFoundError, UnicodeDecodeError):
            current = ""
            existed = False
    else:
        current = ""

    new_lines = [
        f"- [{title}]({path})" for path, title in new_entries
    ]

    if _INDEX_SECTION_HEADER in current:
        # Append below the existing section header, deduping paths
        # that are already present.
        lines = current.splitlines()
        out: list[str] = []
        in_section = False
        section_end = None
        already_present: set[str] = set()
        for i, line in enumerate(lines):
            if line.strip() == _INDEX_SECTION_HEADER:
                in_section = True
                out.append(line)
                continue
            if in_section and line.startswith("## "):
                # Section break — dump new entries just before it.
                section_end = i
                break
            if in_section and line.startswith("- "):
                already_present.add(line.strip())
            out.append(line)
        # If we hit a section break we still have the rest of the
        # file to re-append.
        rest = lines[section_end:] if section_end is not None else []
        filtered_new = [
            ln for ln in new_lines if ln not in already_present
        ]
        # Trim a single trailing blank line in the section so the new
        # entries sit flush under the existing ones.
        while out and out[-1] == "":
            out.pop()
        out.extend(filtered_new)
        if rest:
            out.append("")
            out.extend(rest)
        content = "\n".join(out)
        if not content.endswith("\n"):
            content += "\n"
    else:
        header = "# Library index\n\n" if not current else ""
        preface = current
        if preface and not preface.endswith("\n"):
            preface += "\n"
        if preface and not preface.endswith("\n\n"):
            preface += "\n"
        content = (
            header
            + preface
            + _INDEX_SECTION_HEADER
            + "\n\n"
            + "\n".join(new_lines)
            + "\n"
        )

    sandbox.write("index.md", content)
    return existed


# ─── Provenance sidecar (13-13) ───────────────────────────────────────


def _load_provenance(sandbox: LibrarySandbox) -> dict[str, Any]:
    """Load and return the ``.provenance.json`` sidecar.

    Story 13-13 — FR40 source provenance tracking. Returns the parsed
    JSON object on success, or an empty dict (``{}``) when the sidecar
    is absent, unreadable, corrupt, or structurally invalid.

    Corruption handling is intentionally permissive: the ingest skill
    must never crash on a damaged sidecar, because the next successful
    ingest rewrites all affected slugs and restores a well-formed
    sidecar. Callers that want strict validation can round-trip the
    return value through :func:`_update_provenance` (which accepts any
    dict shape and normalizes it). A corruption-path warning is logged
    to the module logger once per call so operators can notice the
    damage, but the pipeline proceeds.

    The sidecar schema (version 1) is:

    .. code-block:: json

        {
          "version": 1,
          "entries": {
            "<slug>": {
              "source": "...",
              "source_type": "text|markdown|pdf",
              "first_ingest_date": "YYYY-MM-DDTHH:MM:SSZ",
              "last_ingest_date": "YYYY-MM-DDTHH:MM:SSZ",
              "history": ["YYYY-MM-DDTHH:MM:SSZ", ...]
            }
          }
        }

    Future schema versions will bump ``version`` and extend ``entries``
    with additional fields; ``_load_provenance`` will continue to
    return the raw dict so old readers gracefully ignore unknown keys.
    """
    try:
        exists = sandbox.exists(_PROVENANCE_PATH)
    except Exception:  # noqa: BLE001 — robust against mock gaps
        return {}
    if not exists:
        return {}
    try:
        raw = sandbox.read(_PROVENANCE_PATH)
    except (FileNotFoundError, UnicodeDecodeError, OSError):
        logger.warning(
            "library_ingest: could not read %s — returning empty sidecar",
            _PROVENANCE_PATH,
        )
        return {}
    try:
        parsed = json.loads(raw)
    except (json.JSONDecodeError, ValueError):
        logger.warning(
            "library_ingest: %s is not valid JSON — returning empty sidecar",
            _PROVENANCE_PATH,
        )
        return {}
    if not isinstance(parsed, dict):
        logger.warning(
            "library_ingest: %s top-level is not a JSON object — "
            "returning empty sidecar",
            _PROVENANCE_PATH,
        )
        return {}
    entries = parsed.get("entries")
    if entries is not None and not isinstance(entries, dict):
        logger.warning(
            "library_ingest: %s 'entries' field is not an object — "
            "returning empty sidecar",
            _PROVENANCE_PATH,
        )
        return {}
    return parsed


def _update_provenance(
    existing: Mapping[str, Any],
    entries: list[tuple[str, str, str]],
    ingest_ts: str,
) -> dict[str, Any]:
    """Return a new sidecar dict with ``entries`` folded into ``existing``.

    Story 13-13 — pure function (does not mutate ``existing``). Each
    ``(slug, source, source_type)`` tuple in ``entries`` produces or
    refreshes an entry in the returned sidecar:

    - **New slug** — a fresh entry with
      ``first_ingest_date == last_ingest_date == ingest_ts`` and
      ``history == [ingest_ts]``.
    - **Existing slug** — preserve ``first_ingest_date`` from the prior
      entry, overwrite ``source`` + ``source_type`` (the re-ingest's
      values), set ``last_ingest_date = ingest_ts``, and append
      ``ingest_ts`` to ``history``.

    The returned dict's ``entries`` mapping is rebuilt in slug-sorted
    order so the JSON serialization is deterministic (AC-12 byte
    stability). Entries present in ``existing`` but not touched by this
    update are carried forward unchanged.
    """
    # Defensive deep-ish copy — we do not want to mutate the caller's
    # dict, and we must not carry forward a reference to nested lists
    # (``history``) that the caller might hold.
    prior_entries_raw = existing.get("entries") if isinstance(
        existing, Mapping
    ) else None
    prior_entries: dict[str, dict[str, Any]] = {}
    if isinstance(prior_entries_raw, Mapping):
        for slug, value in prior_entries_raw.items():
            if not isinstance(slug, str) or not isinstance(value, Mapping):
                continue
            history_raw = value.get("history", [])
            if isinstance(history_raw, list):
                history_copy = [
                    str(h) for h in history_raw if isinstance(h, str)
                ]
            else:
                history_copy = []
            prior_entries[slug] = {
                "source": value.get("source", ""),
                "source_type": value.get("source_type", ""),
                "first_ingest_date": value.get("first_ingest_date", ""),
                "last_ingest_date": value.get("last_ingest_date", ""),
                "history": history_copy,
            }

    # Apply the updates. Sorted iteration keeps the output stable when
    # two entries touch the same slug (practically impossible because
    # _check_ingest_consistency already rejects duplicate slugs, but
    # cheap and defensive).
    for slug, source, source_type in sorted(entries):
        if slug in prior_entries:
            entry = prior_entries[slug]
            entry["source"] = source
            entry["source_type"] = source_type
            entry["last_ingest_date"] = ingest_ts
            entry["history"] = [*entry.get("history", []), ingest_ts]
        else:
            prior_entries[slug] = {
                "source": source,
                "source_type": source_type,
                "first_ingest_date": ingest_ts,
                "last_ingest_date": ingest_ts,
                "history": [ingest_ts],
            }

    sorted_entries: dict[str, dict[str, Any]] = {
        slug: prior_entries[slug] for slug in sorted(prior_entries)
    }
    return {
        "version": _PROVENANCE_SCHEMA_VERSION,
        "entries": sorted_entries,
    }


def _serialize_provenance(sidecar: Mapping[str, Any]) -> str:
    """Return a stable JSON serialization of ``sidecar``.

    Story 13-13 — uses ``sort_keys=True`` so git diffs are legible and
    ``indent=2`` so the sidecar is human-readable in a text editor.
    ``ensure_ascii=False`` preserves non-ASCII source names as UTF-8,
    matching how :func:`_render_page` emits JSON strings in the YAML
    front-matter. A trailing newline is appended so POSIX tools do not
    complain about the file lacking a final newline.
    """
    return (
        json.dumps(
            sidecar, sort_keys=True, indent=2, ensure_ascii=False
        )
        + "\n"
    )


def get_provenance(
    sandbox: LibrarySandbox, slug: str
) -> dict[str, Any] | None:
    """Return the provenance entry for ``slug``, or ``None`` if absent.

    Story 13-13 — public helper for downstream Library consumers
    (13-15 citations, 13-24 status, 13-25 list, future audit tooling).
    Loads the sidecar via :func:`_load_provenance` on each call — there
    is no caching because the caller knows its own read cadence and
    the sidecar is a few KB even for a 500-page Library.

    Returns a dict with keys ``source``, ``source_type``,
    ``first_ingest_date``, ``last_ingest_date``, ``history``, or
    ``None`` if either the sidecar is missing / empty or the slug has
    no entry.
    """
    sidecar = _load_provenance(sandbox)
    entries = sidecar.get("entries") if isinstance(sidecar, dict) else None
    if not isinstance(entries, dict):
        return None
    entry = entries.get(slug)
    if not isinstance(entry, dict):
        return None
    return dict(entry)


# ─── Registry ─────────────────────────────────────────────────────────

#: Handler signature for any Library skill registered in ``SKILL_REGISTRY``.
LibrarySkillHandler = Callable[..., SkillResult]

#: Single source of truth for the dispatch layer in ``agent_claude.loop``.
#: Adding a new Library skill (13-14 retrieval, 13-16 lint) is a one-line
#: addition to this dict. The dispatch side only imports this mapping.
#:
#: Story 13-18 adds ``library_lint``. The import is done at the bottom of
#: this module (not at the top) to avoid any import-order surprise — the
#: lint module does not depend on this module, so the one-way import is
#: safe, but the deferred form keeps the registry declaration co-located
#: with the entry it registers.
from wheelhouse.skills.library_lint import (  # noqa: E402
    SKILL_NAME as LIBRARY_LINT_SKILL_NAME,
    run_library_lint,
)

SKILL_REGISTRY: dict[str, LibrarySkillHandler] = {
    SKILL_NAME: run_library_ingest,
    LIBRARY_LINT_SKILL_NAME: run_library_lint,
}


__all__ = [
    "ACCEPTED_SOURCE_TYPES",
    "LIBRARY_DISABLED",
    "LIBRARY_INGEST_BINARY_REJECTED",
    "LIBRARY_INGEST_INCONSISTENT_CROSS_REFS",
    "LIBRARY_INGEST_INCONSISTENT_DUPLICATE_SLUG",
    "LIBRARY_INGEST_INCONSISTENT_MISSING_PROVENANCE",
    "LIBRARY_INGEST_INCONSISTENT_SLUG_COLLISION",
    "LIBRARY_INGEST_INCONSISTENT_UNSAFE_SLUG",
    "LIBRARY_INGEST_INVALID_ARGS",
    "LIBRARY_INGEST_LIBRARY_FULL",
    "LIBRARY_INGEST_NO_SUMMARIZER",
    "LIBRARY_INGEST_NOT_IMPLEMENTED",
    "LIBRARY_INGEST_PDF_EMPTY_EXTRACTION",
    "LIBRARY_INGEST_PDF_INVALID",
    "LIBRARY_INGEST_SOURCE_NOT_FOUND",
    "LIBRARY_INGEST_SOURCE_TOO_LARGE",
    "LIBRARY_INGEST_SUMMARIZER_FAILED",
    "LIBRARY_INGEST_UNSUPPORTED_TYPE",
    "LIBRARY_READ_ONLY",
    "MAX_INGEST_WORDS",
    "OPTIONAL_PARAMS",
    "PAGE_COUNT_BLOCK",
    "PAGE_COUNT_WARN",
    "PageDraft",
    "REQUIRED_PARAMS",
    "SKILL_NAME",
    "SKILL_REGISTRY",
    "Summarizer",
    "SummarizerResult",
    "get_provenance",
    "get_summarizer",
    "run_library_ingest",
    "set_summarizer",
]
