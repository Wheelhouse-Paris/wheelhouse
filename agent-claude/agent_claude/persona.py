"""Persona file loading for agent-claude.

Loads SOUL.md, IDENTITY.md, and MEMORY.md from the persona volume.
Missing files are handled gracefully per AC-03:
  - SOUL.md / IDENTITY.md absent: warn + empty string, do NOT exit
  - MEMORY.md absent: create empty file, warn, do NOT exit
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path

logger = logging.getLogger("agent_claude")


@dataclass
class Persona:
    """Loaded persona content from the /persona volume."""

    soul: str
    identity: str
    memory: str

    def build_system_prompt(self) -> str:
        """Build the system prompt by concatenating persona files.

        Format: SOUL + '\\n\\n' + IDENTITY + '\\n\\n' + MEMORY
        Per ADR-017.
        """
        return f"{self.soul}\n\n{self.identity}\n\n{self.memory}"


def load_persona(persona_path: str) -> Persona:
    """Load persona files from the given directory path.

    Args:
        persona_path: Path to directory containing SOUL.md, IDENTITY.md, MEMORY.md.

    Returns:
        Persona dataclass with loaded content.

    Behavior:
        - Existing files: loaded, debug log with path + byte count
        - Missing SOUL.md / IDENTITY.md: warn, use empty string
        - Missing MEMORY.md: create empty file, warn (Story 2.5 consistency)
    """
    base = Path(persona_path)

    soul = _load_file(base / "SOUL.md", create_if_missing=False)
    identity = _load_file(base / "IDENTITY.md", create_if_missing=False)
    memory = _load_file(base / "MEMORY.md", create_if_missing=True)

    return Persona(soul=soul, identity=identity, memory=memory)


def _load_file(path: Path, *, create_if_missing: bool) -> str:
    """Load a single persona file.

    Args:
        path: Full path to the persona file.
        create_if_missing: If True, create an empty file when absent (MEMORY.md behavior).

    Returns:
        File content as string, or empty string if absent.
    """
    if path.exists():
        content = path.read_text(encoding="utf-8")
        byte_count = len(content.encode("utf-8"))
        logger.debug("Loaded persona file %s (%d bytes)", path, byte_count)
        return content

    if create_if_missing:
        logger.warning(
            "Persona file %s not found — initialized empty file", path.name
        )
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
    else:
        logger.warning(
            "Persona file %s not found — using empty string", path.name
        )

    return ""
