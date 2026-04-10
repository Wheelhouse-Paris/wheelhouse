"""LibrarySandbox — structural filesystem boundary for Library skills.

Every Library filesystem operation (read, write, list, delete, exists)
is routed through a :class:`LibrarySandbox` instance bound to a single
Library root. Path canonicalization via :func:`os.path.realpath` plus a
:func:`os.path.commonpath` containment check ensures that skill code
cannot escape the root — neither via ``..`` traversal, nor via symlinks
inside the root, nor via absolute-path injection, nor via
sibling-prefix attacks (e.g. ``/tmp/libA`` vs ``/tmp/libA-evil``).

See:
    - epics-library.md#Story FW-1.1
    - architecture.md#ADR-036 (Library Workspace Volume and Mount Strategy)
    - prd.md FR38, NFR7, NFR9

Public API:
    LibrarySandbox(library_root)
        .read(rel_path) -> str
        .write(rel_path, content) -> None
        .list(rel_path=".") -> list[str]
        .delete(rel_path) -> None
        .exists(rel_path) -> bool

All operations raise :class:`wheelhouse.errors.PathEscapeError` when the
requested path resolves outside the Library root. The error string is
sanitized and does not contain the attempted raw path (NFR9).
"""

from __future__ import annotations

import hashlib
import os
from typing import Union

from wheelhouse.errors import PathEscapeError

PathLike = Union[str, "os.PathLike[str]"]


def _hash_path(canonical: str) -> str:
    """Return a short hex digest of a canonical path for diagnostics only.

    Used by :class:`PathEscapeError` to correlate repeated escape attempts
    without ever emitting the raw path to logs (NFR9).
    """
    return hashlib.sha256(canonical.encode("utf-8", errors="replace")).hexdigest()[:12]


class LibrarySandbox:
    """Filesystem sandbox bound to a single Library root.

    Construction canonicalizes the provided ``library_root`` via
    :func:`os.path.realpath`, so passing a symlink resolves to the real
    target directory. The root must exist and must be a directory.

    Args:
        library_root: Filesystem path to the Library root. Must exist and
            be a directory.

    Raises:
        FileNotFoundError: if the resolved root does not exist.
        ValueError: if the resolved root is not a directory.
    """

    def __init__(self, library_root: PathLike) -> None:
        resolved = os.path.realpath(os.fspath(library_root))
        if not os.path.exists(resolved):
            raise FileNotFoundError(
                f"Library root does not exist: {resolved}"
            )
        if not os.path.isdir(resolved):
            raise ValueError(f"Library root is not a directory: {resolved}")
        self._root: str = resolved

    # ─── Public API ────────────────────────────────────────────────────

    def read(self, rel_path: str) -> str:
        """Read a UTF-8 text file inside the Library root."""
        target = self._validate(rel_path)
        with open(target, "r", encoding="utf-8") as f:
            return f.read()

    def write(self, rel_path: str, content: str) -> None:
        """Write a UTF-8 text file inside the Library root.

        Parent directories are created as needed, but only *after* the
        path has been validated — an escape attempt never creates any
        directory on disk.
        """
        target = self._validate(rel_path)
        parent = os.path.dirname(target)
        if parent and not os.path.isdir(parent):
            os.makedirs(parent, exist_ok=True)
        with open(target, "w", encoding="utf-8") as f:
            f.write(content)

    def list(self, rel_path: str = ".") -> list[str]:
        """Recursively list file paths under ``rel_path`` (relative to root).

        Returns file paths relative to the Library root, sorted
        lexicographically. Directory entries are skipped — only files
        appear in the result. ``rel_path`` must resolve inside the root.
        """
        target = self._validate(rel_path)
        if not os.path.isdir(target):
            # Consistent with os.listdir: if the validated path exists but
            # is not a directory, raise NotADirectoryError.
            raise NotADirectoryError(f"Not a directory: {rel_path}")
        results: list[str] = []
        for dirpath, _dirnames, filenames in os.walk(target):
            for name in filenames:
                abs_path = os.path.join(dirpath, name)
                rel = os.path.relpath(abs_path, self._root)
                # Normalize to forward slashes for deterministic output on
                # all platforms (Library pages are logically POSIX paths).
                results.append(rel.replace(os.sep, "/"))
        results.sort()
        return results

    def delete(self, rel_path: str) -> None:
        """Remove a file inside the Library root."""
        target = self._validate(rel_path)
        os.remove(target)

    def exists(self, rel_path: str) -> bool:
        """Return True iff ``rel_path`` resolves to an existing path inside the root.

        Note: escape attempts still raise :class:`PathEscapeError` for
        consistency with the other methods — this method cannot be used
        to silently probe for paths outside the sandbox.
        """
        target = self._validate(rel_path)
        return os.path.exists(target)

    # ─── Internals ─────────────────────────────────────────────────────

    def _validate(self, rel_path: str) -> str:
        """Resolve ``rel_path`` against the root and enforce containment.

        Returns the canonical absolute path on success. Raises
        :class:`PathEscapeError` on any attempt to escape the root.
        Raises :class:`ValueError` on empty input.
        """
        if not isinstance(rel_path, str) or rel_path == "":
            # Empty path is nonsensical — reject structurally. This is
            # consistent with the rest of the API (no silent pass-through).
            raise ValueError("rel_path must be a non-empty string")

        # os.path.join handles absolute paths by discarding the prefix —
        # i.e. join("/root", "/etc/passwd") == "/etc/passwd". That is
        # intentional here: it means realpath will then canonicalize the
        # absolute path outside the root, and the containment check below
        # rejects it.
        candidate = os.path.realpath(os.path.join(self._root, rel_path))

        # Containment via commonpath — NOT startswith. A naive
        # startswith check would allow ``/tmp/libA-evil`` to bypass a
        # root of ``/tmp/libA`` (sibling-prefix attack).
        try:
            common = os.path.commonpath([candidate, self._root])
        except ValueError:
            # commonpath raises ValueError if the paths are on different
            # drives (Windows) or mix absolute/relative. Any such case is
            # an escape.
            raise PathEscapeError(debug_hash=_hash_path(candidate)) from None

        if common != self._root:
            raise PathEscapeError(debug_hash=_hash_path(candidate))

        return candidate
