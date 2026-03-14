"""Claude Code CLI wrapper for agent-claude.

Calls `claude -p --output-format json` as a subprocess so the agent
uses the user's Claude Max subscription via OAuth, without requiring
a direct Anthropic API key.

Authentication: set CLAUDE_CODE_OAUTH_TOKEN in the container environment
(via `wh secrets init`).

Session continuity: each conversation_id (typically user_id) gets its own
persistent Claude Code session, resumed via --resume <session_id>.
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

    Maintains per-conversation sessions via --resume <session_id> so each
    user gets persistent conversation history. The persona (system prompt)
    is injected via --append-system-prompt on the first turn only; resumed
    sessions inherit the conversation context from Claude Code's session store.

    Authentication uses CLAUDE_CODE_OAUTH_TOKEN env var.
    """

    def __init__(self) -> None:
        # conversation_id (e.g. user_id) -> claude session_id
        self._sessions: dict[str, str] = {}

    async def complete(
        self,
        system_prompt: str,
        user_message: str,
        *,
        timeout: float = 60.0,
        msg_type: str = "unknown",
        stream_name: str = "unknown",
        conversation_id: str = "default",
    ) -> CompletionResult | None:
        """Run `claude -p --output-format json` and return the result.

        On the first call for a conversation_id, injects the system prompt via
        --append-system-prompt and starts a new session. Subsequent calls use
        --resume <session_id> to continue the same conversation.

        Args:
            system_prompt: Persona content — injected only on first turn.
            user_message: The user turn content.
            timeout: Maximum seconds to wait for the subprocess.
            msg_type: Message type for logging.
            stream_name: Stream name for logging.
            conversation_id: Key for session continuity (typically user_id).

        Returns:
            CompletionResult with the response text, or None on timeout/error.

        Raises:
            ClaudeAuthError: If the CLI reports an authentication failure.
        """
        start_time = time.monotonic()
        session_id = self._sessions.get(conversation_id)

        cmd = ["claude", "-p", "--output-format", "json", "--dangerously-skip-permissions"]

        if session_id:
            cmd += ["--resume", session_id]
        else:
            cmd += ["--append-system-prompt", system_prompt]

        cmd.append(user_message)

        def _call() -> Any:
            import os
            env = os.environ.copy()
            env.pop("CLAUDECODE", None)  # prevent nested-session detection
            return subprocess.run(
                cmd,
                capture_output=True,
                stdin=subprocess.DEVNULL,
                text=True,
                timeout=timeout,
                env=env,
                cwd="/tmp",
            )

        try:
            proc = await asyncio.wait_for(
                asyncio.to_thread(_call),
                timeout=timeout + 5,
            )
        except (asyncio.TimeoutError, subprocess.TimeoutExpired):
            elapsed = time.monotonic() - start_time
            logger.error(
                "claude -p timed out: type=%s stream=%s conv=%s elapsed=%.1fs",
                msg_type, stream_name, conversation_id, elapsed,
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
                "claude -p failed: type=%s stream=%s conv=%s elapsed=%.1fs rc=%d stderr=%s",
                msg_type, stream_name, conversation_id, elapsed, proc.returncode, stderr[:200],
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
                "claude -p error result: type=%s stream=%s conv=%s error=%s",
                msg_type, stream_name, conversation_id, error_msg[:200],
            )
            return None

        # Store session_id for conversation continuity
        new_session_id = data.get("session_id")
        if new_session_id and new_session_id != session_id:
            self._sessions[conversation_id] = new_session_id
            logger.debug(
                "claude -p session %s: conv=%s",
                "started" if not session_id else "rotated",
                conversation_id,
            )

        text = data.get("result", "")
        logger.debug(
            "claude -p completed: type=%s stream=%s conv=%s elapsed=%.1fs chars=%d session=%s",
            msg_type, stream_name, conversation_id, elapsed, len(text),
            "resumed" if session_id else "new",
        )
        return CompletionResult(text=text, input_tokens=0, output_tokens=0)
