"""Persona file loading for agent-claude.

Loads SOUL.md, IDENTITY.md, and MEMORY.md from the persona volume.
Missing files are handled gracefully per AC-03:
  - SOUL.md / IDENTITY.md absent: warn + empty string, do NOT exit
  - MEMORY.md absent: create empty file, warn, do NOT exit
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from pathlib import Path

logger = logging.getLogger("agent_claude")


@dataclass
class Persona:
    """Loaded persona content from the /persona volume."""

    soul: str
    identity: str
    memory: str
    stream_contexts: dict[str, str] = field(default_factory=dict)

    def build_system_prompt(self) -> str:
        """Build the system prompt by concatenating persona files and stream contexts.

        Format: SOUL + '\\n\\n' + IDENTITY + '\\n\\n' + MEMORY + stream context sections
        Per ADR-017 and ADR-021.

        Stream context sections are appended in alphabetical order by stream name.
        Each section has a markdown header: '## Stream Context: <stream_name>'
        """
        prompt = f"{self.soul}\n\n{self.identity}\n\n{self.memory}"

        for stream_name in sorted(self.stream_contexts):
            content = self.stream_contexts[stream_name]
            prompt += f"\n\n## Stream Context: {stream_name}\n\n{content}"

        return prompt

    def reload_memory(self, persona_path: str) -> None:
        """Re-read MEMORY.md from disk before each Claude API call (AC-04).

        MEMORY.md may be updated by the agent via git commit (FR-62).
        SOUL.md and IDENTITY.md are read once at startup and cached.
        """
        memory_file = Path(persona_path) / "MEMORY.md"
        if memory_file.exists():
            content = memory_file.read_text(encoding="utf-8")
            byte_count = len(content.encode("utf-8"))
            logger.debug("Reloaded MEMORY.md (%d bytes)", byte_count)
            self.memory = content
        else:
            logger.debug("MEMORY.md not found during reload — using empty string")
            self.memory = ""


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
