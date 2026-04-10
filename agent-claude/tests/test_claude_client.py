"""Unit tests for ClaudeClient — Story 13.2 (workspace cwd, ADR-036).

These tests mock ``subprocess.run`` so no real ``claude`` CLI is launched.
The sole purpose of this file (at v0.1 scope) is to lock in the
ADR-036 cwd contract: the ``claude -p`` subprocess must start with
``cwd="/workspace/"`` — not ``/tmp``, not ``/workspace/.library/``.
"""

from __future__ import annotations

import json
import subprocess
from types import SimpleNamespace

import pytest

from agent_claude.claude_client import ClaudeClient


class _FakeCompletedProcess:
    """Minimal stand-in for ``subprocess.CompletedProcess``."""

    def __init__(self) -> None:
        self.returncode = 0
        self.stdout = json.dumps(
            {
                "result": "ok",
                "session_id": "sess-abc",
                "is_error": False,
            }
        )
        self.stderr = ""


class _SubprocessRunSpy:
    """Captures the kwargs passed to ``subprocess.run``."""

    def __init__(self) -> None:
        self.calls: list[dict] = []

    def __call__(self, *args, **kwargs) -> _FakeCompletedProcess:
        self.calls.append(
            {
                "args": args,
                "kwargs": kwargs,
            }
        )
        return _FakeCompletedProcess()


@pytest.mark.asyncio
async def test_complete_runs_subprocess_with_workspace_cwd(monkeypatch):
    """AC-4 (Story 13.2, ADR-036):

    ``ClaudeClient.complete()`` must launch the ``claude -p`` subprocess with
    ``cwd="/workspace/"`` — never ``/tmp`` (the old value) and never
    ``/workspace/.library/`` (explicitly forbidden by ADR-036 because it
    would put the schema file outside ``LibrarySandbox`` boundaries).
    """
    spy = _SubprocessRunSpy()
    monkeypatch.setattr(
        "agent_claude.claude_client.subprocess.run",
        spy,
    )

    client = ClaudeClient()
    result = await client.complete(
        system_prompt="persona",
        user_message="hello",
        timeout=5.0,
        msg_type="TextMessage",
        stream_name="main",
        conversation_id="user-1",
    )

    assert result is not None
    assert result.text == "ok"
    assert len(spy.calls) == 1

    cwd = spy.calls[0]["kwargs"].get("cwd")
    assert cwd == "/workspace/", (
        f"ADR-036: claude subprocess cwd must be '/workspace/' (got {cwd!r})"
    )
    # Defensive: lock in the two explicit anti-patterns from ADR-036.
    assert cwd != "/tmp"
    assert cwd != "/workspace/.library/"


@pytest.mark.asyncio
async def test_complete_cwd_is_workspace_on_resumed_session(monkeypatch):
    """The cwd contract holds on both the first call (session create) and
    the resume call (session already known). We verify the second call to
    make sure the cwd argument is not conditional on session state.
    """
    spy = _SubprocessRunSpy()
    monkeypatch.setattr(
        "agent_claude.claude_client.subprocess.run",
        spy,
    )

    client = ClaudeClient()
    await client.complete(
        system_prompt="persona",
        user_message="hello",
        timeout=5.0,
        conversation_id="user-resume",
    )
    await client.complete(
        system_prompt="persona",
        user_message="again",
        timeout=5.0,
        conversation_id="user-resume",
    )

    assert len(spy.calls) == 2
    for call in spy.calls:
        assert call["kwargs"].get("cwd") == "/workspace/"
