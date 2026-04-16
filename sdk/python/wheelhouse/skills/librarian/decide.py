"""Core decision function for the librarian (ADR-046).

``decide()`` is the single entry point shared by both framework and cloud
runtimes.  The ``llm_fn`` parameter abstracts the LLM backend so the same
logic runs everywhere.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

from wheelhouse.skills.librarian.prompts import get_prompt
from wheelhouse.skills.librarian.types import DecisionResult

if TYPE_CHECKING:
    from wheelhouse.skills.librarian.types import (
        ConversationMessage,
        LibraryState,
        LlmFn,
    )


def _build_user_content(
    segment: list[ConversationMessage],
    library_state: LibraryState,
) -> str:
    """Format the conversation segment and library context for the LLM."""
    lines: list[str] = ["## CONVERSATION SEGMENT\n"]
    for msg in segment:
        lines.append(f"**{msg.role}**: {msg.content}\n")

    lines.append("\n## LIBRARY INDEX\n")
    lines.append(library_state.index_content or "(empty)")

    if library_state.existing_pages:
        lines.append("\n\n## EXISTING PAGES\n")
        for path, content in library_state.existing_pages.items():
            lines.append(f"### {path}\n```\n{content}\n```\n")

    return "\n".join(lines)


def _parse_llm_response(raw: str) -> dict:
    """Parse the LLM JSON response, stripping optional markdown fences."""
    text = raw.strip()
    # Strip ```json ... ``` fences if present.
    if text.startswith("```"):
        first_nl = text.index("\n")
        last_fence = text.rfind("```")
        text = text[first_nl + 1 : last_fence].strip()
    return json.loads(text)


def decide(
    segment: list[ConversationMessage],
    library_state: LibraryState,
    locale: str,
    llm_fn: LlmFn,
) -> DecisionResult:
    """Evaluate a conversation segment and decide whether to write.

    Parameters
    ----------
    segment:
        The conversation turn to evaluate.
    library_state:
        Current Library state (index + existing pages).
    locale:
        ISO 639-1 locale code (``"en"``, ``"fr"``, ...).  Falls back to
        English when the locale is unsupported or empty.
    llm_fn:
        ``(system_prompt, user_content) -> response_text``.  Injected by the
        runtime — framework wraps ``anthropic.Anthropic().messages.create()``,
        cloud wraps Bedrock ``invoke_model()``.

    Returns
    -------
    DecisionResult
        The librarian's decision with reason code and optional page content.
    """
    effective_locale = locale if locale else "en"
    system_prompt = get_prompt(effective_locale)
    user_content = _build_user_content(segment, library_state)

    try:
        raw_response = llm_fn(system_prompt, user_content)
    except Exception:
        return DecisionResult(reason="decision_error", committed=False)

    try:
        parsed = _parse_llm_response(raw_response)
    except (json.JSONDecodeError, ValueError):
        return DecisionResult(reason="decision_error", committed=False)

    reason = parsed.get("reason", "decision_error")
    committed = parsed.get("committed", False)
    page_path = parsed.get("page_path")
    content = parsed.get("content")

    try:
        return DecisionResult(
            reason=reason,
            committed=committed,
            page_path=page_path if committed else None,
            content=content if committed else None,
            schema_version=1,
        )
    except ValueError:
        # LLM returned an unknown reason code — treat as decision_error.
        return DecisionResult(reason="decision_error", committed=False)
