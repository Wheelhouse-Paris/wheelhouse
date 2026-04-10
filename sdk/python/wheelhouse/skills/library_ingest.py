"""Library ingest skill — scaffolding only (Story 13-7).

This module registers the ``library_ingest`` skill with the Wheelhouse
Library skill dispatch path and delivers the invocation shell:

  * the parameter schema and argument validator,
  * the plan-state gate (``WH_LIBRARY_STATUS`` read-only / disabled),
  * the error-code catalogue consumed by ``SkillResult.error_code``,
  * the extension seam ``_ingest_pipeline`` that 13-8 / 13-9 will
    replace with the real text / markdown / PDF content logic.

The happy-path content pipeline is intentionally deferred — this story
ships the shell so that 13-8 (text/markdown), 13-9 (PDF), 13-10 (size
limits), 13-11 (consistency check), 13-12 (dedup) and 13-13 (provenance)
can parallelize on a frozen invocation contract. Calling the scaffold
today returns a clean ``SkillResult(success=False,
error_code="LIBRARY_INGEST_NOT_IMPLEMENTED")`` after argument validation
has passed.

See:
    - _bmad-output/planning-artifacts/wh/epics-library.md FW-3.1
    - _bmad-output/planning-artifacts/wh/architecture.md ADR-038
    - _bmad-output/implementation-artifacts/wh/13-7-library-ingest-skill-scaffolding.md
    - wheelhouse.errors.LibrarySkillError (frozen code catalogue below)
"""

from __future__ import annotations

import logging
from typing import Any, Callable, Mapping

from wheelhouse.errors import LibrarySkillError
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

#: Scaffold sentinel — argument validation passed but 13-7 does not yet
#: implement the content pipeline. Stories 13-8 (text/markdown) and 13-9
#: (PDF) replace ``_ingest_pipeline`` with real logic and this code will
#: no longer be produced on the happy path.
LIBRARY_INGEST_NOT_IMPLEMENTED = "LIBRARY_INGEST_NOT_IMPLEMENTED"


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

# Scaffold sentinel. Mentions the follow-up story IDs so any downstream
# consumer (dashboard, billing drilldown) can link a user question back
# to the relevant ticket.
_NOT_IMPLEMENTED_MESSAGE = (
    "library_ingest scaffolding only — content pipeline arrives in "
    "stories 13-8 (text/markdown) and 13-9 (PDF)."
)


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

    # Step 5 — delegate to the content pipeline. 13-7 only: this always
    # raises NOT_IMPLEMENTED. 13-8 and 13-9 replace the function body so
    # ``run_library_ingest`` never has to change.
    try:
        return _ingest_pipeline(sandbox, parameters)
    except LibrarySkillError as exc:
        return SkillResult(
            invocation_id=invocation_id,
            skill_name=skill_name,
            success=False,
            error_code=exc.code or LIBRARY_INGEST_NOT_IMPLEMENTED,
            error_message=str(exc),
        )


def _ingest_pipeline(
    sandbox: LibrarySandbox,
    parameters: Mapping[str, str],
) -> SkillResult:
    """Scaffold-only delegate — 13-8 / 13-9 replace the body.

    In 13-7 this function unconditionally raises
    ``LibrarySkillError(LIBRARY_INGEST_NOT_IMPLEMENTED)``. The signature is
    frozen:

    * ``sandbox`` is the boot-constructed ``LibrarySandbox`` (never None
      — ``run_library_ingest`` has already disabled-gated).
    * ``parameters`` has been validated: required keys present,
      ``source_type`` is a member of ``ACCEPTED_SOURCE_TYPES``.

    The future happy path will:

    1. Read source content via ``sandbox.read`` / an SDK fetcher (13-8
       text/markdown; 13-9 PDF extraction).
    2. Generate structured pages via the LLM (13-8).
    3. Open a transaction with ``sandbox.transaction("ingest", summary)``,
       write pages, populate ``commit_metadata`` for the ADR-039 commit
       message, and return a ``SkillResult(success=True, ...)`` with
       ``library_page_count`` / ``library_last_ingest_at`` / ``library_tokens``
       populated per 13-27.
    4. Size-limit, consistency, dedup and provenance layers plug in
       around this body without touching ``run_library_ingest``.
    """
    raise LibrarySkillError(
        _NOT_IMPLEMENTED_MESSAGE,
        code=LIBRARY_INGEST_NOT_IMPLEMENTED,
    )


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
    "LIBRARY_INGEST_INVALID_ARGS",
    "LIBRARY_INGEST_NOT_IMPLEMENTED",
    "LIBRARY_READ_ONLY",
    "OPTIONAL_PARAMS",
    "REQUIRED_PARAMS",
    "SKILL_NAME",
    "SKILL_REGISTRY",
    "run_library_ingest",
]
