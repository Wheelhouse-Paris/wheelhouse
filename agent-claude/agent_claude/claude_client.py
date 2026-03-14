"""Claude Code CLI wrapper for agent-claude.

Calls `claude -p --output-format json` as a subprocess so the agent
uses the user's Claude Max subscription via OAuth, without requiring
a direct Anthropic API key.

Authentication: set CLAUDE_CODE_OAUTH_TOKEN in the container environment
(via `wh secrets set CLAUDE_CODE_OAUTH_TOKEN <token>`).
"""

from __future__ import annotations

import asyncio
import json
import logging
import subprocess
import time
from dataclasses import dataclass
from typing import Any

from agent_claude.errors import ClaudeAuthError

logger = logging.getLogger("agent_claude")


@dataclass
class CompletionResult:
    """Result from a Claude completion call."""

    text: str
    input_tokens: int
    output_tokens: int


class ClaudeClient:
    """Calls `claude -p --output-format json` as a subprocess.

    Each call spawns a fresh `claude` process. The system prompt (persona)
    is injected via --append-system-prompt. No session is persisted between
    calls so the persona is always applied fresh.

    Authentication uses CLAUDE_CODE_OAUTH_TOKEN env var or the credentials
    stored in ~/.claude/ inside the container.
    """

    def __init__(self) -> None:
        pass  # auth is handled by the claude CLI itself

    async def complete(
        self,
        system_prompt: str,
        user_message: str,
        *,
        timeout: float = 60.0,
        msg_type: str = "unknown",
        stream_name: str = "unknown",
    ) -> CompletionResult | None:
        """Run `claude -p --output-format json` and return the result.

        Args:
            system_prompt: Persona content appended to Claude's system prompt.
            user_message: The user turn content.
            timeout: Maximum seconds to wait for the subprocess.
            msg_type: Message type for logging (TextMessage, CronEvent, etc.).
            stream_name: Stream name for logging.

        Returns:
            CompletionResult with the response text, or None on timeout/error.
            Token counts are 0 — not reported by the CLI.

        Raises:
            ClaudeAuthError: If the CLI reports an authentication failure.
        """
        start_time = time.monotonic()

        cmd = [
            "claude", "-p",
            "--output-format", "json",
            "--dangerously-skip-permissions",
            "--no-session-persistence",
            "--append-system-prompt", system_prompt,
            user_message,
        ]

        def _call() -> Any:
            import os
            env = os.environ.copy()
            env.pop("CLAUDECODE", None)  # prevent nested-session detection
            return subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=timeout,
                env=env,
            )

        try:
            proc = await asyncio.wait_for(
                asyncio.to_thread(_call),
                timeout=timeout + 5,
            )
        except (asyncio.TimeoutError, subprocess.TimeoutExpired):
            elapsed = time.monotonic() - start_time
            logger.error(
                "claude -p timed out: type=%s stream=%s elapsed=%.1fs",
                msg_type, stream_name, elapsed,
            )
            return None

        elapsed = time.monotonic() - start_time

        if proc.returncode != 0:
            stderr = proc.stderr.strip()
            if any(kw in stderr.lower() for kw in ("authentication", "unauthorized", "oauth", "401")):
                raise ClaudeAuthError(
                    "agent-claude: Claude Code authentication failed "
                    "-- check CLAUDE_CODE_OAUTH_TOKEN"
                )
            logger.warning(
                "claude -p failed: type=%s stream=%s elapsed=%.1fs rc=%d stderr=%s",
                msg_type, stream_name, elapsed, proc.returncode, stderr[:200],
            )
            return None

        try:
            data = json.loads(proc.stdout)
        except json.JSONDecodeError:
            logger.warning(
                "claude -p non-JSON output: type=%s stream=%s output=%s",
                msg_type, stream_name, proc.stdout[:200],
            )
            return None

        if data.get("is_error"):
            error_msg = data.get("result", "unknown error")
            if any(kw in error_msg.lower() for kw in ("auth", "oauth", "unauthorized")):
                raise ClaudeAuthError(
                    "agent-claude: Claude Code authentication failed "
                    "-- check CLAUDE_CODE_OAUTH_TOKEN"
                )
            logger.warning(
                "claude -p error result: type=%s stream=%s error=%s",
                msg_type, stream_name, error_msg[:200],
            )
            return None

        text = data.get("result", "")
        logger.debug(
            "claude -p completed: type=%s stream=%s elapsed=%.1fs chars=%d",
            msg_type, stream_name, elapsed, len(text),
        )
        return CompletionResult(text=text, input_tokens=0, output_tokens=0)
