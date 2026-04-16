"""Wheelhouse Librarian — decision policy for autonomous Library writes.

This package implements the librarian's core decision logic (ADR-046).
The ``decide()`` function evaluates a conversation segment and returns
a ``DecisionResult`` indicating whether to write, update, or skip.

The ``llm_fn`` injection point abstracts the LLM backend so the same
code runs on both framework (Anthropic API) and cloud (Bedrock) runtimes.
"""

from wheelhouse.skills.librarian.decide import decide
from wheelhouse.skills.librarian.types import (
    ConversationMessage,
    DecisionResult,
    LibraryState,
)

__all__ = [
    "ConversationMessage",
    "DecisionResult",
    "LibraryState",
    "decide",
]
