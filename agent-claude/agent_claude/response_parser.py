"""Batch response parser for agent-claude (ADR-022).

Parses the JSON array output format from the LLM and provides the
system prompt instruction that tells the LLM to use this format.

Expected format:
    [{"stream": "<name>", "type": "TextMessage", "content": "<text>"}, ...]

Empty array [] is a valid no-op response.
"""

from __future__ import annotations

import json
import logging
import re

logger = logging.getLogger("agent_claude")

# Required keys and their expected types for each batch item
_REQUIRED_KEYS = {"stream": str, "type": str, "content": str}

# Currently supported message types
_SUPPORTED_TYPES = {"TextMessage"}

# Template for the batch output instruction injected into the system prompt.
# The {stream_list} placeholder is replaced with the comma-separated stream names.
_BATCH_OUTPUT_INSTRUCTION_TEMPLATE = """\
## Output Format

You MUST always respond with a raw JSON array (no markdown fences, no commentary outside the array).
Each element is an object with exactly three keys:
- "stream": the target stream name (one of: {stream_list})
- "type": "TextMessage"
- "content": your response text

To send no response, return an empty array: []

Example (single response to stream "main"):
[{{"stream": "main", "type": "TextMessage", "content": "Hello!"}}]

Example (multi-stream):
[{{"stream": "main", "type": "TextMessage", "content": "Update posted."}}, {{"stream": "logs", "type": "TextMessage", "content": "Action logged."}}]

IMPORTANT: Output ONLY the JSON array. No text before or after it."""


def format_batch_instruction(streams: list[str]) -> str:
    """Build the batch output instruction with the given stream names.

    Args:
        streams: List of stream names the agent is subscribed to.

    Returns:
        Formatted instruction string for inclusion in the system prompt.
    """
    stream_list = ", ".join(streams) if streams else "(none)"
    return _BATCH_OUTPUT_INSTRUCTION_TEMPLATE.format(stream_list=stream_list)


def _strip_code_fences(text: str) -> str:
    """Strip markdown code fences and surrounding text if present.

    Handles ```json ... ``` and ``` ... ``` wrapping that LLMs
    sometimes produce despite instructions not to.  Also tolerates
    trailing commentary after the closing fence (a common LLM habit).
    """
    stripped = text.strip()
    # Match ```json\n...\n``` with optional trailing text
    m = re.search(
        r"```(?:json)?\s*\n(.*?)```", stripped, re.DOTALL
    )
    if m:
        return m.group(1).strip()
    return stripped


def parse_batch_response(raw_text: str) -> list[dict] | None:
    """Parse the LLM response as a JSON array of batch output items.

    Args:
        raw_text: Raw text from the LLM response (CompletionResult.text).

    Returns:
        List of dicts on success (may be empty for no-op).
        None if the response is malformed (invalid JSON, wrong structure,
        missing keys, etc.). The entire batch is rejected on any
        validation error to prevent partial publishes.
    """
    text = _strip_code_fences(raw_text)

    # Parse JSON
    try:
        data = json.loads(text)
    except (json.JSONDecodeError, TypeError):
        logger.error(
            "Batch response is not valid JSON: %.200s", raw_text
        )
        return None

    # Must be a list
    if not isinstance(data, list):
        logger.error(
            "Batch response is not a JSON array (got %s): %.200s",
            type(data).__name__,
            raw_text,
        )
        return None

    # Empty array is a valid no-op
    if len(data) == 0:
        return []

    # Validate each item
    for i, item in enumerate(data):
        if not isinstance(item, dict):
            logger.error(
                "Batch response item %d is not an object (got %s): %.200s",
                i,
                type(item).__name__,
                raw_text,
            )
            return None

        for key, expected_type in _REQUIRED_KEYS.items():
            if key not in item:
                logger.error(
                    "Batch response item %d missing required key '%s': %.200s",
                    i,
                    key,
                    raw_text,
                )
                return None
            if not isinstance(item[key], expected_type):
                logger.error(
                    "Batch response item %d key '%s' has wrong type "
                    "(expected %s, got %s): %.200s",
                    i,
                    key,
                    expected_type.__name__,
                    type(item[key]).__name__,
                    raw_text,
                )
                return None

        # Validate non-empty strings
        if not item["stream"]:
            logger.error(
                "Batch response item %d has empty 'stream': %.200s",
                i,
                raw_text,
            )
            return None

        if not item["content"]:
            logger.error(
                "Batch response item %d has empty 'content': %.200s",
                i,
                raw_text,
            )
            return None

        # Validate supported type
        if item["type"] not in _SUPPORTED_TYPES:
            logger.error(
                "Batch response item %d has unsupported type '%s' "
                "(supported: %s): %.200s",
                i,
                item["type"],
                ", ".join(_SUPPORTED_TYPES),
                raw_text,
            )
            return None

    return data
