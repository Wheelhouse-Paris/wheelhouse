"""Library lint skill — detection pipeline (Story 13-16).

This module delivers the *detection core* for the ``library.lint`` skill.
It does NOT register anything with the skill dispatch path (that is
13-18) and it does NOT expose a CLI (that is 13-26). Incremental mode —
re-linting only pages that changed since the last lint — is 13-17.

Detection surface
-----------------

Four detectors, two pure and two LLM-backed:

* :func:`detect_orphans` — pure, graph-based. A page is an orphan iff no
  other page lists it in its ``cross_refs`` front-matter field. The
  Library's root ``index.md`` is never flagged (it is the table of
  contents, references flow outward from it).
* :func:`detect_stale_refs` — pure. A cross-reference is stale iff its
  target page path is absent from the scanned page set.
* :func:`detect_contradictions` — LLM-backed. Delegates the pairwise
  comparison work to an injected ``llm_fn`` callable so unit tests can
  substitute a mock. 13-18 binds the production ``llm_fn`` to the agent
  runtime's LLM client.
* :func:`detect_outdated_claims` — LLM-backed, same injection pattern.

Orchestrator
------------

:func:`lint_library` takes a :class:`LibrarySandbox`, parses every
``*.md`` file in the Library (optionally filtered by ``page_filter``),
runs each detector, and returns a single flat ``list[LintFinding]``. It
is tolerant of a missing ``llm_fn`` — the pure detectors still run and
the LLM-backed ones are skipped with a DEBUG log.

Page format
-----------

Minimal YAML front-matter contract per the 13-19 default-schema
template:

::

    ---
    source: ...
    ingest_date: 2026-04-10
    cross_refs: [other.md, clients/acme.md]
    ---
    # Body

Only ``cross_refs`` is load-bearing for detection — the other keys are
parsed into ``front_matter`` verbatim and echoed but not interpreted.

Why a hand-rolled parser? Story 13-7 did not add PyYAML to the SDK's
dependency surface and pulling in a new runtime dependency here is out
of scope. The fields the detectors need (``cross_refs`` as a flow list)
are simple enough to extract with a regex-free line scanner. If 13-17
needs richer parsing it can swap to PyYAML in a follow-up.

See:
    - _bmad-output/planning-artifacts/wh/epics-library.md FW-5.1 (FR20)
    - _bmad-output/planning-artifacts/wh/architecture.md
    - _bmad-output/implementation-artifacts/wh/13-16-lint-detection-pipeline.md
"""

from __future__ import annotations

import dataclasses
import logging
from typing import Any, Callable, Iterable, Mapping, Sequence

from wheelhouse.skills.library_sandbox import LibrarySandbox

logger = logging.getLogger("wheelhouse.library_lint")


# ─── Finding categories (frozen string enum) ──────────────────────────

#: A claim in one page directly contradicts a claim in another page.
#: Detected by :func:`detect_contradictions` via LLM comparison.
CATEGORY_CONTRADICTION = "contradiction"

#: A page is not referenced from ``index.md`` or any other page.
#: Detected by :func:`detect_orphans`, pure graph walk.
CATEGORY_ORPHAN = "orphan"

#: A cross-reference points to a page that no longer exists in the
#: Library. Detected by :func:`detect_stale_refs`, pure.
CATEGORY_STALE_REF = "stale_ref"

#: A date-sensitive claim the LLM judges may be outdated.
#: Heuristic only — FR20 explicitly accepts false positives in v1.
CATEGORY_OUTDATED_CLAIM = "outdated_claim"


# ─── Severity enum ────────────────────────────────────────────────────

SEVERITY_INFO = "info"
SEVERITY_WARN = "warn"
SEVERITY_ERROR = "error"


# ─── Dataclasses ──────────────────────────────────────────────────────


@dataclasses.dataclass(frozen=True)
class LintFinding:
    """A single lint finding.

    ``LintFinding`` is a value object — frozen so that detectors can
    freely share instances across orchestration layers without risk of
    in-place mutation. The fields are deliberately minimal: 13-26 (CLI)
    and 13-18 (conversation) are expected to render them into their own
    richer report formats without needing any extra structured context
    beyond (category, page, detail, severity).

    Attributes:
        category: one of the ``CATEGORY_*`` constants.
        page: path to the offending page, relative to the Library root.
            May be an empty string for cross-page findings where no
            single page is primary (the LLM-backed detectors still
            pick one representative page in practice).
        detail: human-readable explanation suitable for operator logs
            and CLI output. MAY reference other pages by path.
        severity: one of the ``SEVERITY_*`` constants.
    """

    category: str
    page: str
    detail: str
    severity: str


@dataclasses.dataclass(frozen=True)
class LibraryPage:
    """Parsed representation of a single Library page.

    Produced by :func:`parse_pages`. Detectors only read
    ``.path``, ``.body``, ``.cross_refs`` — ``.front_matter`` is the
    unparsed bag of additional YAML keys kept for forward compatibility
    with 13-8's evolving schema.
    """

    path: str
    body: str
    cross_refs: tuple[str, ...]
    front_matter: Mapping[str, Any]


# ─── Front-matter parser (minimal, no PyYAML) ─────────────────────────

_FRONT_MATTER_DELIMITER = "---"


def _parse_front_matter(text: str) -> tuple[dict[str, Any], str]:
    """Split a markdown document into (front_matter, body).

    Accepts the standard YAML front-matter block delimited by ``---``
    on its own line at the start of the file. If no front-matter block
    is present, returns ``({}, text)``.

    Only the subset of YAML used by the Library schema template is
    supported: ``key: value`` scalars and ``key: [a, b, c]`` flow
    lists. Nested mappings and block-style lists are left as raw
    strings in the ``front_matter`` dict — the detectors only care
    about ``cross_refs`` which is always a flow list per the template.
    """
    if not text.startswith(_FRONT_MATTER_DELIMITER):
        return {}, text

    lines = text.splitlines(keepends=True)
    # First line must be exactly "---\n" (or "---" at EOF).
    if not (lines and lines[0].rstrip("\r\n") == _FRONT_MATTER_DELIMITER):
        return {}, text

    # Find closing delimiter.
    close_idx = -1
    for i in range(1, len(lines)):
        if lines[i].rstrip("\r\n") == _FRONT_MATTER_DELIMITER:
            close_idx = i
            break
    if close_idx == -1:
        # Unterminated front-matter block — treat as no front-matter
        # rather than crashing. A 13-17 richer parser can tighten this.
        return {}, text

    fm_lines = lines[1:close_idx]
    body = "".join(lines[close_idx + 1 :])
    # Strip one leading newline from body for cleanliness.
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
        front_matter[key] = _parse_scalar_or_list(value)
    return front_matter, body


def _parse_scalar_or_list(value: str) -> Any:
    """Parse ``"[a, b]"`` → ``["a","b"]``; otherwise return the scalar."""
    if value.startswith("[") and value.endswith("]"):
        inner = value[1:-1].strip()
        if not inner:
            return []
        parts = [p.strip().strip("'").strip('"') for p in inner.split(",")]
        return [p for p in parts if p]
    # Unquote a simple quoted scalar.
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        return value[1:-1]
    return value


# ─── parse_pages ──────────────────────────────────────────────────────


def parse_pages(
    sandbox: LibrarySandbox,
    page_filter: Iterable[str] | None = None,
) -> list[LibraryPage]:
    """Read and parse every ``*.md`` file in the Library.

    Iterates via ``sandbox.list()`` (not direct filesystem access) so
    the LibrarySandbox containment boundary from 13-1..13-6 is never
    bypassed. Files outside ``page_filter`` (when given) are skipped
    without being read — this matters for 13-17 incremental mode where
    the filter is the "changed since last lint" set.

    Files that fail to parse are logged and skipped; the detection
    pipeline MUST NOT crash on a single malformed page.
    """
    filter_set: set[str] | None = None
    if page_filter is not None:
        filter_set = {str(p) for p in page_filter}

    pages: list[LibraryPage] = []
    for rel in sandbox.list("."):
        if not rel.endswith(".md"):
            continue
        if filter_set is not None and rel not in filter_set:
            continue
        try:
            raw = sandbox.read(rel)
        except Exception as exc:  # pragma: no cover - defensive
            logger.warning("library_lint: failed to read %s: %s", rel, exc)
            continue

        front_matter, body = _parse_front_matter(raw)
        cross_refs_raw = front_matter.get("cross_refs", [])
        if isinstance(cross_refs_raw, str):
            cross_refs_raw = [cross_refs_raw]
        if not isinstance(cross_refs_raw, list):
            cross_refs_raw = []
        cross_refs = tuple(str(r) for r in cross_refs_raw if r)

        pages.append(
            LibraryPage(
                path=rel,
                body=body,
                cross_refs=cross_refs,
                front_matter=front_matter,
            )
        )
    return pages


# ─── Pure detectors ───────────────────────────────────────────────────


def _is_index_page(path: str) -> bool:
    """True if ``path`` is the root ``index.md`` (never flagged as orphan)."""
    return path == "index.md"


def detect_orphans(pages: Sequence[LibraryPage]) -> list[LintFinding]:
    """Flag pages with no inbound cross-references.

    Pure graph walk: for every page P, check whether any *other* page
    lists P.path in its cross_refs. If none do, and P is not the root
    ``index.md``, flag it.
    """
    inbound: set[str] = set()
    for p in pages:
        for ref in p.cross_refs:
            inbound.add(ref)

    findings: list[LintFinding] = []
    for p in pages:
        if _is_index_page(p.path):
            continue
        if p.path in inbound:
            continue
        findings.append(
            LintFinding(
                category=CATEGORY_ORPHAN,
                page=p.path,
                detail=(
                    f"page {p.path!r} is not referenced from any other "
                    "page; consider linking it from index.md or a topic page"
                ),
                severity=SEVERITY_WARN,
            )
        )
    return findings


def detect_stale_refs(pages: Sequence[LibraryPage]) -> list[LintFinding]:
    """Flag cross-references whose target page does not exist in the set."""
    known: set[str] = {p.path for p in pages}
    findings: list[LintFinding] = []
    for p in pages:
        for ref in p.cross_refs:
            if ref not in known:
                findings.append(
                    LintFinding(
                        category=CATEGORY_STALE_REF,
                        page=p.path,
                        detail=(
                            f"cross-reference to {ref!r} is stale — "
                            "target page not found in the Library"
                        ),
                        severity=SEVERITY_WARN,
                    )
                )
    return findings


# ─── LLM-backed detectors ─────────────────────────────────────────────

#: Callable shape for the injected LLM detector function. The detector
#: name is passed as the second arg so a single implementation can route
#: to the right prompt template in 13-18.
LlmDetectorFn = Callable[[Sequence[LibraryPage], str], list[dict]]


def detect_contradictions(
    pages: Sequence[LibraryPage],
    llm_fn: LlmDetectorFn,
) -> list[LintFinding]:
    """Flag contradictions across pages via the injected LLM function.

    ``llm_fn`` is expected to return a list of dicts of the shape::

        {"pages": ["a.md", "b.md"], "detail": "price differs"}

    Each verdict produces ONE :class:`LintFinding` attached to the
    first page in the verdict's ``pages`` list (the detail string names
    the others). Empty / malformed verdicts are skipped.
    """
    try:
        verdicts = llm_fn(pages, CATEGORY_CONTRADICTION) or []
    except Exception as exc:  # pragma: no cover - defensive
        logger.warning("library_lint: contradictions llm_fn failed: %s", exc)
        return []

    findings: list[LintFinding] = []
    for v in verdicts:
        if not isinstance(v, dict):
            continue
        involved = v.get("pages") or []
        if not involved:
            continue
        primary = str(involved[0])
        detail = str(v.get("detail") or "contradiction detected")
        findings.append(
            LintFinding(
                category=CATEGORY_CONTRADICTION,
                page=primary,
                detail=detail,
                severity=SEVERITY_ERROR,
            )
        )
    return findings


def detect_outdated_claims(
    pages: Sequence[LibraryPage],
    llm_fn: LlmDetectorFn,
) -> list[LintFinding]:
    """Flag date-sensitive claims the LLM judges potentially outdated.

    ``llm_fn`` verdict shape::

        {"page": "news.md", "detail": "as of 2024 ..."}

    Severity is ``info`` because FR20 accepts heuristic false positives
    in v1 — outdated-claim findings should nudge but never block.
    """
    try:
        verdicts = llm_fn(pages, CATEGORY_OUTDATED_CLAIM) or []
    except Exception as exc:  # pragma: no cover - defensive
        logger.warning("library_lint: outdated-claims llm_fn failed: %s", exc)
        return []

    findings: list[LintFinding] = []
    for v in verdicts:
        if not isinstance(v, dict):
            continue
        page = v.get("page")
        if not page:
            continue
        detail = str(v.get("detail") or "date-sensitive claim may be outdated")
        findings.append(
            LintFinding(
                category=CATEGORY_OUTDATED_CLAIM,
                page=str(page),
                detail=detail,
                severity=SEVERITY_INFO,
            )
        )
    return findings


# ─── Orchestrator ─────────────────────────────────────────────────────


def lint_library(
    sandbox: LibrarySandbox,
    llm_fn: LlmDetectorFn | None = None,
    page_filter: Iterable[str] | None = None,
) -> list[LintFinding]:
    """Run every detector over the Library and return a flat findings list.

    Pure detectors (orphan, stale-ref) always run. LLM-backed detectors
    (contradiction, outdated-claim) run only when ``llm_fn`` is
    provided — callers without an LLM binding (unit tests, 13-26 CLI
    with ``--no-llm``) still get the structural findings.

    ``page_filter`` — when given, only pages whose relative path is in
    the filter are parsed. 13-17 will pass the "changed since last
    lint" set here to implement incremental mode without modifying the
    detector signatures.

    This function never raises: a malformed page is logged and skipped;
    a failing ``llm_fn`` is logged and contributes zero findings. The
    caller (13-18 conversation path, 13-26 CLI) is expected to surface
    the returned list; empty list == clean Library.
    """
    pages = parse_pages(sandbox, page_filter=page_filter)
    findings: list[LintFinding] = []

    findings.extend(detect_orphans(pages))
    findings.extend(detect_stale_refs(pages))

    if llm_fn is not None:
        findings.extend(detect_contradictions(pages, llm_fn))
        findings.extend(detect_outdated_claims(pages, llm_fn))
    else:
        logger.debug(
            "library_lint: llm_fn not provided — "
            "skipping contradiction + outdated-claim detectors"
        )

    return findings


__all__ = [
    "CATEGORY_CONTRADICTION",
    "CATEGORY_ORPHAN",
    "CATEGORY_OUTDATED_CLAIM",
    "CATEGORY_STALE_REF",
    "LibraryPage",
    "LintFinding",
    "SEVERITY_ERROR",
    "SEVERITY_INFO",
    "SEVERITY_WARN",
    "detect_contradictions",
    "detect_orphans",
    "detect_outdated_claims",
    "detect_stale_refs",
    "lint_library",
    "parse_pages",
]
