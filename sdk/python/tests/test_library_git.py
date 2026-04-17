"""Acceptance tests for LibraryGit per-file git history SDK (Stories 15-1-2, 15-1-3).

Covers ADR-050 constraints LE-03 (path validation), LE-04 (binary safety).
All tests require the ``git`` CLI on PATH.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path
from typing import Iterator

import pytest

from wheelhouse.errors import LibraryCommitError, PathEscapeError
from wheelhouse.skills.library_git import (
    LibraryGit,
    LogEntry,
    SearchResult,
    TreeEntry,
    _validate_sha,
)
from wheelhouse.skills.library_sandbox import LibrarySandbox

# ─── Skip the whole file if `git` is not on PATH ──────────────────────
pytestmark = pytest.mark.skipif(
    shutil.which("git") is None,
    reason="git CLI not available — required by LibraryGit",
)


# ─── Fixtures ─────────────────────────────────────────────────────────


@pytest.fixture()
def library_root(tmp_path: Path) -> Path:
    root = tmp_path / "library"
    root.mkdir()
    return root


@pytest.fixture()
def isolated_git_env(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Iterator[None]:
    """Run tests with NO ambient git config."""
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    monkeypatch.delenv("GIT_AUTHOR_NAME", raising=False)
    monkeypatch.delenv("GIT_AUTHOR_EMAIL", raising=False)
    monkeypatch.delenv("GIT_COMMITTER_NAME", raising=False)
    monkeypatch.delenv("GIT_COMMITTER_EMAIL", raising=False)
    yield


@pytest.fixture()
def git_sandbox(
    library_root: Path, isolated_git_env: None
) -> LibrarySandbox:
    return LibrarySandbox(
        str(library_root), git_enabled=True, agent_name="alice"
    )


@pytest.fixture()
def library_git(git_sandbox: LibrarySandbox) -> LibraryGit:
    return LibraryGit(git_sandbox)


def _commit_file(
    sandbox: LibrarySandbox,
    rel_path: str,
    content: str,
    operation: str = "ingest",
    summary: str = "test commit",
) -> str:
    """Helper: write a text file and commit it. Returns the commit SHA."""
    with sandbox.transaction(operation, summary) as _handle:
        sandbox.write(rel_path, content)
    # Get the HEAD SHA
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=sandbox._root,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def _commit_binary(
    sandbox: LibrarySandbox,
    rel_path: str,
    content: bytes,
    operation: str = "ingest",
    summary: str = "test binary commit",
) -> str:
    """Helper: write binary content and commit. Returns the commit SHA."""
    with sandbox.transaction(operation, summary) as _handle:
        sandbox.write_bytes(rel_path, content)
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=sandbox._root,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


# ─── AC #1: tree() at HEAD returns entries with path, type, size ──────


class TestTree:
    def test_tree_at_head_returns_entries(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #1: tree() returns entries with path, type, size, last_modified."""
        _commit_file(git_sandbox, "pages/overview.md", "# Overview\nHello world")
        _commit_file(git_sandbox, "pages/detail.md", "# Detail\nMore content here")

        entries = library_git.tree()

        assert len(entries) == 2
        paths = {e.path for e in entries}
        assert paths == {"pages/overview.md", "pages/detail.md"}

        for entry in entries:
            assert entry.type == "file"
            assert entry.size > 0
            assert entry.last_modified > 0

    def test_tree_entries_are_sorted(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """tree() returns entries sorted by path."""
        _commit_file(git_sandbox, "b.md", "B")
        _commit_file(git_sandbox, "a.md", "A")

        entries = library_git.tree()

        assert [e.path for e in entries] == ["a.md", "b.md"]

    def test_tree_empty_repo(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """tree() on a repo with no tracked files returns empty list."""
        # Initialize the repo with an empty commit.
        git_sandbox._ensure_git_repo()
        subprocess.run(
            ["git", "commit", "--allow-empty", "-m", "init"],
            cwd=git_sandbox._root,
            capture_output=True,
            check=True,
            env={
                **os.environ,
                "GIT_AUTHOR_NAME": "test",
                "GIT_AUTHOR_EMAIL": "test@test",
                "GIT_COMMITTER_NAME": "test",
                "GIT_COMMITTER_EMAIL": "test@test",
            },
        )

        entries = library_git.tree()
        assert entries == []

    # ── AC #2: tree(sha=...) returns historical state ──────────────

    def test_tree_at_historical_sha(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #2: tree(sha=...) reflects the repo state at that commit."""
        sha1 = _commit_file(git_sandbox, "pages/first.md", "First")
        _commit_file(git_sandbox, "pages/second.md", "Second")

        # At sha1, only first.md should exist
        entries = library_git.tree(sha=sha1)
        paths = {e.path for e in entries}
        assert "pages/first.md" in paths
        assert "pages/second.md" not in paths

    def test_tree_at_historical_sha_includes_deleted_files(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #2: Files deleted since that commit appear in the historical tree."""
        sha1 = _commit_file(git_sandbox, "pages/will-delete.md", "content")
        # Delete the file
        with git_sandbox.transaction("lint", "remove stale page"):
            git_sandbox.delete("pages/will-delete.md")

        # At sha1, the deleted file should still appear
        entries = library_git.tree(sha=sha1)
        paths = {e.path for e in entries}
        assert "pages/will-delete.md" in paths

        # At HEAD, it should NOT appear
        entries = library_git.tree()
        paths = {e.path for e in entries}
        assert "pages/will-delete.md" not in paths


# ─── AC #3, #4, #5: file_content() ───────────────────────────────────


class TestFileContent:
    def test_file_content_text_returns_bytes(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #3: file_content returns raw UTF-8 bytes for text files."""
        _commit_file(git_sandbox, "pages/fiscal.md", "# Fiscal Year\nData here")

        content = library_git.file_content("pages/fiscal.md")

        assert isinstance(content, bytes)
        assert content == b"# Fiscal Year\nData here"

    def test_file_content_binary_returns_raw_bytes(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #4: file_content returns raw binary bytes for PDF/binary files."""
        binary_data = bytes(range(256)) * 10  # non-UTF8 binary
        _commit_binary(git_sandbox, "sources/report.pdf", binary_data)

        content = library_git.file_content("sources/report.pdf")

        assert isinstance(content, bytes)
        assert content == binary_data

    def test_file_content_at_historical_sha(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #5: file_content(sha=...) returns content at that commit."""
        sha1 = _commit_file(git_sandbox, "pages/evolving.md", "version 1")
        _commit_file(git_sandbox, "pages/evolving.md", "version 2")

        # At HEAD, should be version 2
        content_head = library_git.file_content("pages/evolving.md")
        assert content_head == b"version 2"

        # At sha1, should be version 1
        content_v1 = library_git.file_content("pages/evolving.md", sha=sha1)
        assert content_v1 == b"version 1"

    def test_file_content_nonexistent_raises_fnf(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """file_content raises FileNotFoundError for missing files."""
        _commit_file(git_sandbox, "pages/exists.md", "present")

        with pytest.raises(FileNotFoundError, match="does not exist"):
            library_git.file_content("pages/no-such-file.md")


# ─── AC #6: Path traversal raises PathEscapeError ────────────────────


class TestPathSecurity:
    def test_file_content_path_traversal_raises(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #6: Path traversal attempt raises PathEscapeError."""
        _commit_file(git_sandbox, "pages/legit.md", "ok")

        with pytest.raises(PathEscapeError):
            library_git.file_content("../../etc/passwd")

    def test_file_content_absolute_path_raises(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """Absolute path injection raises PathEscapeError."""
        _commit_file(git_sandbox, "pages/legit.md", "ok")

        with pytest.raises(PathEscapeError):
            library_git.file_content("/etc/passwd")


# ─── SHA validation ──────────────────────────────────────────────────


class TestShaValidation:
    def test_valid_sha_short(self) -> None:
        _validate_sha("abcd")  # 4 chars min

    def test_valid_sha_full(self) -> None:
        _validate_sha("a" * 40)  # 40 chars max

    def test_invalid_sha_too_short(self) -> None:
        with pytest.raises(ValueError, match="Invalid SHA"):
            _validate_sha("abc")  # 3 chars

    def test_invalid_sha_non_hex(self) -> None:
        with pytest.raises(ValueError, match="Invalid SHA"):
            _validate_sha("ghijklmn")

    def test_invalid_sha_command_injection(self) -> None:
        with pytest.raises(ValueError, match="Invalid SHA"):
            _validate_sha("HEAD; rm -rf /")

    def test_tree_invalid_sha_raises(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """tree(sha=...) with invalid SHA raises ValueError."""
        _commit_file(git_sandbox, "pages/x.md", "x")

        with pytest.raises(ValueError, match="Invalid SHA"):
            library_git.tree(sha="not-a-sha!")

    def test_file_content_invalid_sha_raises(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """file_content(sha=...) with invalid SHA raises ValueError."""
        _commit_file(git_sandbox, "pages/x.md", "x")

        with pytest.raises(ValueError, match="Invalid SHA"):
            library_git.file_content("pages/x.md", sha=";;;drop table")


# ─── Story 15-1-3: file_log() ──────────────────────────────────────────


class TestFileLog:
    def test_file_log_returns_all_commits(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #1: file_log returns entries with sha, author, timestamp, message, diff_stat."""
        _commit_file(git_sandbox, "pages/fiscal.md", "v1", summary="initial version")
        _commit_file(git_sandbox, "pages/fiscal.md", "v1\nv2", summary="add line")
        _commit_file(git_sandbox, "pages/fiscal.md", "v1\nv2\nv3", summary="another line")

        entries = library_git.file_log("pages/fiscal.md")

        assert len(entries) == 3
        for entry in entries:
            assert isinstance(entry, LogEntry)
            assert len(entry.sha) == 40
            assert "alice" in entry.author.lower() or "Agent" in entry.author
            assert entry.timestamp > 0
            assert entry.message  # non-empty

    def test_file_log_most_recent_first(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #1: entries are ordered most recent first."""
        _commit_file(git_sandbox, "pages/fiscal.md", "v1", summary="first")
        _commit_file(git_sandbox, "pages/fiscal.md", "v2", summary="second")
        _commit_file(git_sandbox, "pages/fiscal.md", "v3", summary="third")

        entries = library_git.file_log("pages/fiscal.md")

        # Most recent first: timestamps should be non-increasing.
        timestamps = [e.timestamp for e in entries]
        assert timestamps == sorted(timestamps, reverse=True)
        # Most recent message should be "third".
        assert "third" in entries[0].message

    def test_file_log_diff_stat(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #1: diff_stat shows lines added/removed."""
        _commit_file(git_sandbox, "pages/fiscal.md", "line1\nline2\nline3")
        _commit_file(git_sandbox, "pages/fiscal.md", "line1\nchanged\nline3\nline4")

        entries = library_git.file_log("pages/fiscal.md")

        # The most recent commit modified lines — should have a non-empty diff_stat.
        assert entries[0].diff_stat  # e.g. "+2 -1" or similar

    def test_file_log_with_limit(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #2: limit truncates results."""
        for i in range(5):
            _commit_file(
                git_sandbox, "pages/fiscal.md", f"version {i}", summary=f"commit {i}"
            )

        entries = library_git.file_log("pages/fiscal.md", limit=2)

        assert len(entries) == 2

    def test_file_log_nonexistent_file_returns_empty(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """file_log for a path with no commits returns empty list."""
        _commit_file(git_sandbox, "pages/other.md", "content")

        entries = library_git.file_log("pages/no-such-file.md")

        assert entries == []

    def test_file_log_path_traversal_raises(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #5: file_log with path traversal raises PathEscapeError."""
        _commit_file(git_sandbox, "pages/legit.md", "ok")

        with pytest.raises(PathEscapeError):
            library_git.file_log("../../etc/passwd")


# ─── Story 15-1-3: search() ────────────────────────────────────────────


class TestSearch:
    def test_search_returns_matches_with_context(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #3: search returns path, line_number, matching_line, context."""
        content = "line one\nline two\nfiscal year data\nline four\nline five"
        _commit_file(git_sandbox, "pages/report.md", content)

        results = library_git.search("fiscal")

        assert len(results) >= 1
        r = results[0]
        assert isinstance(r, SearchResult)
        assert r.path == "pages/report.md"
        assert r.line_number == 3
        assert "fiscal" in r.matching_line
        # Context: up to 2 lines before and after.
        assert len(r.context_before) <= 2
        assert len(r.context_after) <= 2

    def test_search_matches_in_pages_and_sources(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #3: results include matches in both pages/ and sources/."""
        _commit_file(git_sandbox, "pages/overview.md", "fiscal summary here")
        _commit_file(git_sandbox, "sources/notes.txt", "fiscal notes here")

        results = library_git.search("fiscal")

        matched_paths = {r.path for r in results}
        assert "pages/overview.md" in matched_paths
        assert "sources/notes.txt" in matched_paths

    def test_search_excludes_binary_files(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #3: binary files are excluded from search results."""
        _commit_file(git_sandbox, "pages/overview.md", "fiscal data here")
        # Create a binary file that happens to contain the search string.
        binary_content = b"\x00\x01fiscal\x02\x03" + bytes(range(128, 256))
        _commit_binary(git_sandbox, "sources/report.bin", binary_content)

        results = library_git.search("fiscal")

        matched_paths = {r.path for r in results}
        assert "pages/overview.md" in matched_paths
        assert "sources/report.bin" not in matched_paths

    def test_search_no_matches_returns_empty(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #4: no matches returns empty list, not an error."""
        _commit_file(git_sandbox, "pages/overview.md", "hello world")

        results = library_git.search("xyznonexistent")

        assert results == []

    def test_search_empty_query_returns_empty(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """Empty query returns empty list."""
        _commit_file(git_sandbox, "pages/overview.md", "content")

        results = library_git.search("")

        assert results == []

    def test_search_shell_metacharacters_safe(
        self, git_sandbox: LibrarySandbox, library_git: LibraryGit
    ) -> None:
        """AC #6: shell metacharacters in query are safe (--fixed-strings)."""
        _commit_file(git_sandbox, "pages/overview.md", 'safe content with "quotes"')

        # These should not cause command injection or errors.
        results = library_git.search('"; rm -rf /')
        assert results == []

        results = library_git.search("$(whoami)")
        assert results == []

        results = library_git.search(".*")
        assert results == []  # literal ".*" not in file
