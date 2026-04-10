"""Library sandbox boot wiring for agent-claude (Story 13-7).

This module owns the single ``LibrarySandbox`` instance per agent boot:
it constructs it once, wires ``git_enabled=True`` + the agent identity
from startup config, and calls ``recover_from_crash()`` exactly once
before any skill invocation can reach the dispatch layer.

Degradation rules (NFR24 parity):

* ``library_status == "disabled"`` — no sandbox is constructed, no git
  command is invoked. The ingest handler will refuse invocations with
  ``LIBRARY_DISABLED``.
* ``library_status == "read-only"`` — sandbox IS constructed and
  recovered (so future 13-14 retrieval / 13-16 lint read paths work).
  The ingest handler still refuses writes with ``LIBRARY_READ_ONLY``.
* ``library_status == "enabled"`` — sandbox is constructed, recovered,
  and returned.
* Recovery raising ``LibraryGitError`` — caught, logged as a WARNING,
  and the function returns ``None``. A corrupt Library must not brick
  the whole agent.

See:
    - ADR-036 (Library Workspace Volume)
    - ADR-040 (Concurrent Write Serialization and Crash Recovery)
    - Story 13-5 (recover_from_crash)
    - Story 13-20 (library_status propagation)
    - Story 13-7 (this module)
"""

from __future__ import annotations

import logging
import os
from typing import Any, Mapping

from wheelhouse.errors import LibraryGitError
from wheelhouse.skills.library_sandbox import LibrarySandbox

logger = logging.getLogger("agent_claude")


#: Library root inside the agent's workspace volume.
#:
#: The workspace volume mounts at ``/workspace/`` (ADR-036, story 13-2),
#: and the schema file ``.wh-schema.md`` lives at the workspace root. The
#: Library's git repository is the ``.library/`` subdirectory — kept
#: separate so the schema file is not accidentally committed to the
#: Library history.
LIBRARY_ROOT = "/workspace/.library"


def build_library_sandbox(
    config: Mapping[str, Any],
    *,
    library_root: str = LIBRARY_ROOT,
) -> LibrarySandbox | None:
    """Construct and crash-recover the single per-agent LibrarySandbox.

    Call exactly once during ``run_startup``, after the 13-20 schema
    probe and before ``wheelhouse.connect``. The returned sandbox (or
    ``None``) is attached to ``config["library_sandbox"]`` so the
    dispatch layer in ``agent_claude.loop`` can reach it without a new
    import chain.

    Args:
        config: The startup config dict. Must carry ``library_status``
            (one of ``"enabled" | "read-only" | "disabled"``) and
            ``agent_name``.
        library_root: Library root path override for tests. Defaults to
            the production mount point ``/workspace/.library``.

    Returns:
        The constructed + recovered ``LibrarySandbox``, or ``None`` if
        the Library is disabled or recovery failed. ``None`` means "the
        ingest skill will refuse invocations"; it NEVER means "raise
        and crash the agent".
    """
    library_status = config.get("library_status", "disabled")

    # Disabled → never touch the filesystem or git. The ingest skill
    # will produce a LIBRARY_DISABLED SkillResult on every invocation.
    if library_status == "disabled":
        logger.info(
            "Library disabled — ingest skill will refuse invocations"
        )
        return None

    # Create the Library root if the workspace volume does not already
    # contain a `.library/` directory. Safe because this lives inside
    # the agent's own per-agent volume (ADR-036 / story 13-3). The
    # first ingest must not fail with FileNotFoundError on a clean
    # volume — 13-4's `git init` happens on first commit, but the root
    # directory has to exist before LibrarySandbox can canonicalize it.
    os.makedirs(library_root, exist_ok=True)

    agent_name = str(config.get("agent_name") or "unknown")

    try:
        sandbox = LibrarySandbox(
            library_root,
            git_enabled=True,
            agent_name=agent_name,
        )
    except (FileNotFoundError, ValueError) as exc:
        # FileNotFoundError: library_root vanished between makedirs and
        # realpath (extremely unlikely). ValueError: library_root was
        # not a directory (e.g., a leftover file at that path). Both
        # are degradation cases — log and continue booting.
        logger.warning(
            "LibrarySandbox construction failed — library_ingest "
            "will refuse invocations (reason: %s)",
            type(exc).__name__,
        )
        return None

    try:
        sandbox.recover_from_crash()
    except LibraryGitError as exc:
        # NFR24 parity: a corrupt Library must not brick the agent.
        # The error's `code` attribute (set by the LibraryGitError
        # family) is the operator-facing signal. We log it at WARNING
        # and return None so the dispatch layer degrades to refusal.
        code = getattr(exc, "code", None) or type(exc).__name__
        logger.warning(
            "Library recovery failed: %s: %s — ingest skill will "
            "refuse invocations",
            code,
            exc,
        )
        return None

    logger.info(
        "LibrarySandbox ready — library_status=%s",
        library_status,
    )
    return sandbox
