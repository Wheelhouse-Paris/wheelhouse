"""Claude-backed summarizer for the Library ingest skill (post-Epic 13 wiring).

Adapts the synchronous ``Summarizer`` callable expected by
``wheelhouse.skills.library_ingest`` (Story 13-8) onto the existing
``claude -p`` subprocess pathway used by the rest of agent-claude.

The summarizer is installed exactly once at boot via
:func:`wheelhouse.skills.library_ingest.set_summarizer`. After that, every
``library_ingest`` skill invocation routes its source text through this
module, which:

1. Renders a tightly-scoped JSON-output prompt that lists the existing
   pages (so Claude can decide whether to merge or branch).
2. Shells out to ``claude -p --output-format json`` synchronously
   (matching :class:`agent_claude.claude_client.ClaudeClient`'s subprocess
   contract — same auth path, same cwd, same env hygiene).
3. Parses the JSON envelope and the inner page list.
4. Returns a :class:`wheelhouse.skills.library_ingest.SummarizerResult`.

A page is ``{"path": "<rel>.md", "title": "<H1>", "body": "<markdown>",
"cross_refs": ["<rel>.md", ...]}``. Claude is asked to keep ``body``
short, structured, and non-verbatim (NFR11: never copy raw source text
into the Library — summarize).
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
from typing import Any, Sequence

from wheelhouse.skills.library_ingest import (
    PageDraft,
    SummarizerResult,
)

logger = logging.getLogger("agent_claude")

_SUBPROCESS_TIMEOUT_S = 120.0


_SYSTEM_PROMPT = """\
You are a Library ingest assistant. Your job is to read a source document and
break it down into a small set of structured Library pages.

OUTPUT FORMAT — return a single JSON object with this exact shape:

{
  "summary": "one short sentence describing the ingest, used as the git commit subject",
  "pages": [
    {
      "path": "<short-slug>.md",
      "title": "<page H1 title>",
      "body": "<markdown body, structured, NEVER a verbatim copy of the source>",
      "cross_refs": ["<other-slug>.md", ...]
    }
  ]
}

RULES:
- Output ONLY the JSON object. No prose, no fences, no commentary.
- Each page MUST have a unique "path" ending in ".md", lowercase, slug-only
  (alphanumerics and hyphens, no slashes, no spaces, no dots besides ".md").
- "body" is YOUR summary in YOUR words — never copy long verbatim spans from
  the source. Keep each body under ~600 words.
- "cross_refs" lists other pages in THIS SAME ingest batch by their "path"
  values. Use it sparingly and only when there is a real conceptual link.
- 1 to 5 pages per ingest is the sweet spot. Do not produce a page per
  paragraph.
- If the source is already a Library page (has YAML front-matter), still
  re-summarize it — never copy the front-matter into the body.
"""


def _build_user_prompt(
    source_text: str,
    source_name: str,
    user_summary_hint: str | None,
    existing_pages: Sequence[str],
) -> str:
    parts: list[str] = []
    parts.append(f"SOURCE NAME: {source_name}")
    if user_summary_hint:
        parts.append(f"USER HINT: {user_summary_hint}")
    if existing_pages:
        listed = "\n".join(f"  - {p}" for p in existing_pages[:50])
        parts.append(
            "EXISTING LIBRARY PAGES (for cross-ref context — do not edit):\n"
            f"{listed}"
        )
    parts.append("---SOURCE---")
    parts.append(source_text)
    parts.append("---END SOURCE---")
    parts.append(
        "Return the JSON object now. No prose. No code fences."
    )
    return "\n\n".join(parts)


def _call_claude(system_prompt: str, user_prompt: str) -> tuple[str, int]:
    """Run ``claude -p --output-format json`` synchronously.

    Returns ``(result_text, total_tokens)``. Raises ``RuntimeError`` on
    any failure path so the caller can map to an ingest error code.
    """
    cmd = [
        "claude",
        "-p",
        "--output-format",
        "json",
        "--dangerously-skip-permissions",
        "--append-system-prompt",
        system_prompt,
        user_prompt,
    ]

    env = os.environ.copy()
    env.pop("CLAUDECODE", None)

    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            stdin=subprocess.DEVNULL,
            text=True,
            timeout=_SUBPROCESS_TIMEOUT_S,
            env=env,
            cwd="/workspace/",
        )
    except FileNotFoundError as e:
        raise RuntimeError(f"claude binary not on PATH: {e}") from e
    except subprocess.TimeoutExpired as e:
        raise RuntimeError(
            f"claude -p timed out after {_SUBPROCESS_TIMEOUT_S:.0f}s"
        ) from e

    if proc.returncode != 0:
        stderr = proc.stderr.strip()[:500]
        raise RuntimeError(f"claude -p failed (rc={proc.returncode}): {stderr}")

    try:
        envelope = json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"claude -p returned non-JSON envelope: {e}") from e

    if envelope.get("is_error"):
        raise RuntimeError(
            f"claude -p error envelope: {envelope.get('result', 'unknown')[:300]}"
        )

    text = envelope.get("result", "")
    if not isinstance(text, str) or not text.strip():
        raise RuntimeError("claude -p returned an empty result")

    usage = envelope.get("usage") or {}
    tokens = int(usage.get("input_tokens", 0)) + int(usage.get("output_tokens", 0))

    return text, tokens


def _strip_code_fence(text: str) -> str:
    """Strip a leading/trailing ```json ... ``` fence if Claude added one."""
    s = text.strip()
    if s.startswith("```"):
        # Drop first line (```json or ```)
        s = s.split("\n", 1)[1] if "\n" in s else ""
    if s.endswith("```"):
        s = s.rsplit("```", 1)[0]
    return s.strip()


def _parse_page_list(raw_text: str) -> tuple[list[PageDraft], str]:
    """Parse Claude's JSON response into PageDraft objects.

    Tolerates a leading code fence (Claude occasionally ignores the
    "no code fences" instruction). Returns ``(drafts, summary)``.
    Raises ``RuntimeError`` if the structure is wrong.
    """
    cleaned = _strip_code_fence(raw_text)
    try:
        obj = json.loads(cleaned)
    except json.JSONDecodeError as e:
        raise RuntimeError(f"summarizer returned invalid JSON: {e}") from e

    if not isinstance(obj, dict):
        raise RuntimeError("summarizer JSON top-level must be an object")

    pages = obj.get("pages")
    if not isinstance(pages, list) or not pages:
        raise RuntimeError("summarizer JSON missing non-empty 'pages' array")

    summary = str(obj.get("summary") or "").strip() or "Library ingest"

    drafts: list[PageDraft] = []
    for entry in pages:
        if not isinstance(entry, dict):
            raise RuntimeError("each page must be a JSON object")
        path = str(entry.get("path") or "").strip()
        title = str(entry.get("title") or "").strip()
        body = str(entry.get("body") or "").strip()
        cross_refs_raw = entry.get("cross_refs") or []
        if not isinstance(cross_refs_raw, list):
            raise RuntimeError(f"page {path!r}: cross_refs must be a list")
        cross_refs = [str(c).strip() for c in cross_refs_raw if str(c).strip()]
        if not path or not title or not body:
            raise RuntimeError(
                f"page is missing required field(s): path={path!r}, "
                f"title={title!r}, body_empty={not body}"
            )
        drafts.append(
            PageDraft(path=path, title=title, body=body, cross_refs=cross_refs)
        )

    return drafts, summary


def claude_summarizer(
    source_text: str,
    source_name: str,
    user_summary_hint: str | None,
    existing_pages: list[str],
) -> SummarizerResult:
    """Summarizer callable installed via :func:`set_summarizer` at boot.

    Synchronous (blocks the asyncio loop while ``claude -p`` runs — that's
    fine for a single per-invocation skill call; the surrounding ``loop.py``
    dispatch already serializes one skill invocation at a time per agent).
    """
    user_prompt = _build_user_prompt(
        source_text, source_name, user_summary_hint, existing_pages
    )
    logger.info(
        "library_summarizer: calling claude -p (source=%s, source_chars=%d, existing_pages=%d)",
        source_name,
        len(source_text),
        len(existing_pages),
    )
    raw_text, tokens = _call_claude(_SYSTEM_PROMPT, user_prompt)
    drafts, summary = _parse_page_list(raw_text)
    logger.info(
        "library_summarizer: produced %d page(s), tokens_used=%d",
        len(drafts),
        tokens,
    )
    return SummarizerResult(drafts=drafts, tokens_used=tokens, summary=summary)


def install(_config: Any | None = None) -> None:
    """Wire :func:`claude_summarizer` into the global ingest summarizer slot.

    Called from ``agent_claude.main.run_startup`` after the
    ``LibrarySandbox`` is constructed and before the broker connection is
    established. Idempotent — calling twice replaces the previous install.
    """
    from wheelhouse.skills import library_ingest

    library_ingest.set_summarizer(claude_summarizer)
    logger.info("library_summarizer: claude-backed summarizer installed")
