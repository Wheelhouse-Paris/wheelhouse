"""Library ingest skill — text / markdown happy path (Stories 13-7, 13-8).

This module registers the ``library_ingest`` skill with the Wheelhouse
Library skill dispatch path and delivers both the invocation shell
(13-7 — parameter schema, plan-state gate, error-code catalogue) and,
as of 13-8, the text / markdown content pipeline.

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
OPTIONAL_PARAMS: tuple[str, ...] = ("user_summary_hint",)

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
#: The 4-tuple is
#: ``(source_text, source_name, user_summary_hint, existing_pages)``
#: — ``existing_pages`` is the sorted list of sandbox-relative page
#: paths already present, used by the LLM to decide whether to merge
#: into an existing page or create a new one. 13-12 (dedup) will add
#: more context to this call but MUST NOT change the signature.
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
    if source_type not in ("text", "markdown"):
        # PDF reaches here until 13-9 ships a real branch; URL may
        # never be implemented. Echoing the value is safe because the
        # enum is closed (no NFR9 leakage).
        raise LibrarySkillError(
            _UNSUPPORTED_TYPE_FMT.format(value=source_type),
            code=LIBRARY_INGEST_UNSUPPORTED_TYPE,
        )

    # Step 2 — source resolution. NFR9: no path in the error message.
    source_text, source_name = _resolve_source_text(sandbox, parameters)

    # Step 3 — summarizer wired? Do this BEFORE opening a transaction
    # so AC-9's "sandbox.transaction is never called" assertion holds.
    summarizer = get_summarizer()
    if summarizer is None:
        raise LibrarySkillError(
            _NO_SUMMARIZER_MESSAGE,
            code=LIBRARY_INGEST_NO_SUMMARIZER,
        )

    user_hint_raw = parameters.get("user_summary_hint")
    user_hint = str(user_hint_raw) if user_hint_raw else None
    existing_pages = _list_existing_pages(sandbox)

    # Step 4 — call the summarizer. We wrap ONLY the summarizer call
    # in the try so that a bug inside the writer still surfaces as a
    # real traceback instead of being masked behind SUMMARIZER_FAILED.
    try:
        result = summarizer(source_text, source_name, user_hint, existing_pages)
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

    with sandbox.transaction("ingest", commit_subject) as handle:
        new_page_entries: list[tuple[str, str]] = []  # (path, title)
        for draft in result.drafts:
            page_path = _normalize_page_path(draft.path)
            is_update = sandbox.exists(page_path)
            content = _render_page(
                body=draft.body,
                source_name=source_name,
                ingest_ts=ingest_ts,
                cross_refs=list(draft.cross_refs),
            )
            sandbox.write(page_path, content)
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

    # Step 6 — SkillResult. invocation_id / skill_name are filled in
    # by ``run_library_ingest`` from its own parameters; this function
    # is never called directly by the dispatch layer.
    #
    # ``library_page_count`` is the number of content pages the
    # summarizer emitted (AC-1 pins this to ``len(drafts)``). The
    # auto-written ``index.md`` bookkeeping page is NOT counted — it
    # is skill housekeeping, not user-visible Library content, and the
    # metering Lambda (13-27) treats the count as a proxy for
    # LLM-generated value.
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
    ingest_ts: str,
    cross_refs: list[str],
) -> str:
    """Render a page body with a minimal YAML front-matter block.

    Format pinned by AC-2: three top-level keys in fixed order
    (``source``, ``ingest_date``, ``cross_refs``), then a blank line,
    then the body. ``source`` is emitted as a JSON string so that any
    special characters (quotes, colons) round-trip cleanly without
    pulling in a YAML dependency. ``cross_refs`` is a YAML flow list
    with each item JSON-quoted. The rendered bytes are stable for the
    same inputs.
    """
    if not body.endswith("\n"):
        body = body + "\n"
    refs_yaml = ", ".join(json.dumps(r) for r in cross_refs)
    front_matter = (
        "---\n"
        f"source: {json.dumps(source_name)}\n"
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


# ─── Registry ─────────────────────────────────────────────────────────

#: Handler signature for any Library skill registered in ``SKILL_REGISTRY``.
LibrarySkillHandler = Callable[..., SkillResult]

#: Single source of truth for the dispatch layer in ``agent_claude.loop``.
#: Adding a new Library skill (13-14 retrieval, 13-16 lint) is a one-line
#: addition to this dict. The dispatch side only imports this mapping.
SKILL_REGISTRY: dict[str, LibrarySkillHandler] = {
    SKILL_NAME: run_library_ingest,
}


__all__ = [
    "ACCEPTED_SOURCE_TYPES",
    "LIBRARY_DISABLED",
    "LIBRARY_INGEST_BINARY_REJECTED",
    "LIBRARY_INGEST_INVALID_ARGS",
    "LIBRARY_INGEST_NO_SUMMARIZER",
    "LIBRARY_INGEST_NOT_IMPLEMENTED",
    "LIBRARY_INGEST_SOURCE_NOT_FOUND",
    "LIBRARY_INGEST_SUMMARIZER_FAILED",
    "LIBRARY_INGEST_UNSUPPORTED_TYPE",
    "LIBRARY_READ_ONLY",
    "OPTIONAL_PARAMS",
    "PageDraft",
    "REQUIRED_PARAMS",
    "SKILL_NAME",
    "SKILL_REGISTRY",
    "Summarizer",
    "SummarizerResult",
    "get_summarizer",
    "run_library_ingest",
    "set_summarizer",
]
