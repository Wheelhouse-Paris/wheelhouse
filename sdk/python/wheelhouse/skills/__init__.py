"""Wheelhouse Library skill infrastructure.

This subpackage houses filesystem and skill scaffolding used by Library
skills (ingest, lint, retrieval). Library skill code must access the
filesystem exclusively via :class:`LibrarySandbox` — importing ``os``,
``pathlib``, or ``subprocess`` directly from skill modules is prohibited
by the skill loader (enforced in story 13-7).

Structural filesystem boundary — see ADR-036 in
``_bmad-output/planning-artifacts/wh/architecture.md``.
"""

from wheelhouse.skills.library_sandbox import LibrarySandbox

__all__ = ["LibrarySandbox"]
