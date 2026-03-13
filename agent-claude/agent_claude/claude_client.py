"""Claude API client wrapper for agent-claude.

Wraps the synchronous anthropic.Anthropic client in an async interface
using asyncio.to_thread() per ADR-017. Provides a 60-second timeout
via asyncio.wait_for() (AC-05).
"""

from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass
from typing import Any

import anthropic

from agent_claude.errors import ClaudeAuthError

logger = logging.getLogger("agent_claude")


@dataclass
class CompletionResult:
    """Result from a Claude API completion call."""

    text: str
    input_tokens: int
    output_tokens: int


class ClaudeClient:
    """Async wrapper around the synchronous Anthropic client (ADR-017).

    Uses asyncio.to_thread() for non-blocking API calls and
    asyncio.wait_for() with a 60s timeout to prevent hangs (AC-05).
    """

    def __init__(self, api_key: str, model: str = "claude-3-5-sonnet-20241022") -> None:
        self._client = anthropic.Anthropic(api_key=api_key)
        self.model = model

    async def complete(
        self,
        system_prompt: str,
        user_message: str,
        *,
        timeout: float = 60.0,
        msg_type: str = "unknown",
        stream_name: str = "unknown",
    ) -> CompletionResult | None:
        """Call the Claude API with the given prompts.

        Args:
            system_prompt: The system prompt (persona concatenation).
            user_message: The user turn content.
            timeout: Maximum seconds to wait for API response (AC-05).
            msg_type: Message type for logging (TextMessage, CronEvent, etc.).
            stream_name: Stream name for logging.

        Returns:
            CompletionResult with text and token counts, or None on timeout/transient error.

        Raises:
            ClaudeAuthError: If the API key is invalid (AC-02).
        """
        start_time = time.monotonic()

        def _call() -> Any:
            return self._client.messages.create(
                model=self.model,
                max_tokens=4096,
                system=system_prompt,
                messages=[{"role": "user", "content": user_message}],
            )

        try:
            response = await asyncio.wait_for(
                asyncio.to_thread(_call),
                timeout=timeout,
            )
            result = CompletionResult(
                text=response.content[0].text,
                input_tokens=response.usage.input_tokens,
                output_tokens=response.usage.output_tokens,
            )
            logger.debug(
                "Claude API call completed: type=%s stream=%s tokens=%d",
                msg_type,
                stream_name,
                result.input_tokens + result.output_tokens,
            )
            return result

        except asyncio.TimeoutError:
            elapsed = time.monotonic() - start_time
            logger.error(
                "Claude API call timed out: type=%s stream=%s elapsed=%.1fs",
                msg_type,
                stream_name,
                elapsed,
            )
            return None

        except anthropic.AuthenticationError as exc:
            raise ClaudeAuthError(
                "agent-claude: Claude API authentication failed "
                "-- check ANTHROPIC_API_KEY"
            ) from exc

        except anthropic.APIError as exc:
            elapsed = time.monotonic() - start_time
            logger.warning(
                "Claude API transient error: type=%s stream=%s elapsed=%.1fs error=%s",
                msg_type,
                stream_name,
                elapsed,
                str(exc),
            )
            return None
