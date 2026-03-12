"""Acceptance tests for SDK documentation accuracy (Story 6-4, AC #3, #4).

These tests verify that documentation code samples match the actual SDK API
and contain no references to nonexistent APIs.
"""

import ast
import re
from pathlib import Path

import pytest

# Project root (two levels up from tests/acceptance/)
PROJECT_ROOT = Path(__file__).parent.parent.parent


class TestSdkPythonDocsAccuracy:
    """Verify docs/src/content/docs/sdk/python.md matches actual SDK API."""

    @pytest.fixture
    def sdk_python_doc(self) -> str:
        path = PROJECT_ROOT / "docs" / "src" / "content" / "docs" / "sdk" / "python.md"
        assert path.exists(), f"SDK Python doc not found at {path}"
        return path.read_text()

    def _extract_python_blocks(self, markdown: str) -> list[str]:
        """Extract all ```python code blocks from markdown."""
        pattern = r"```python\n(.*?)```"
        return re.findall(pattern, markdown, re.DOTALL)

    def test_all_python_blocks_parse(self, sdk_python_doc: str) -> None:
        """Every Python code block in the SDK docs must be valid syntax."""
        blocks = self._extract_python_blocks(sdk_python_doc)
        assert len(blocks) > 0, "No Python code blocks found in SDK docs"
        for i, block in enumerate(blocks):
            try:
                ast.parse(block)
            except SyntaxError as e:
                pytest.fail(f"Python block #{i + 1} has syntax error: {e}\n\nBlock:\n{block}")

    def test_no_base_stream_object_reference(self, sdk_python_doc: str) -> None:
        """Docs must not reference BaseStreamObject (does not exist in SDK)."""
        assert "BaseStreamObject" not in sdk_python_doc, (
            "SDK docs reference 'BaseStreamObject' which does not exist in the SDK"
        )

    def test_no_synchronous_connect(self, sdk_python_doc: str) -> None:
        """Docs must use async connect pattern: 'await wheelhouse.connect()', not 'wh = connect("main")'."""
        # Check for synchronous connect patterns (without await)
        lines = sdk_python_doc.split("\n")
        in_python_block = False
        for line in lines:
            if line.strip().startswith("```python"):
                in_python_block = True
                continue
            if line.strip().startswith("```") and in_python_block:
                in_python_block = False
                continue
            if in_python_block:
                # Lines with connect() should have await (unless it's an import)
                stripped = line.strip()
                if "connect(" in stripped and "import" not in stripped and "await" not in stripped and "#" not in stripped and "def " not in stripped:
                    pytest.fail(
                        f"SDK docs use synchronous connect() without await: '{stripped}'"
                    )

    def test_no_async_connect_alias(self, sdk_python_doc: str) -> None:
        """Docs must not reference 'async_connect' (does not exist)."""
        assert "async_connect" not in sdk_python_doc, (
            "SDK docs reference 'async_connect' which does not exist — connect() is already async"
        )

    def test_no_wh_run(self, sdk_python_doc: str) -> None:
        """Docs must not reference 'wh.run()' (does not exist)."""
        assert "wh.run()" not in sdk_python_doc and ".run()" not in sdk_python_doc, (
            "SDK docs reference '.run()' which does not exist in the SDK"
        )

    def test_no_last_published(self, sdk_python_doc: str) -> None:
        """Docs must not reference 'last_published()' (does not exist; use get_published())."""
        assert "last_published" not in sdk_python_doc, (
            "SDK docs reference 'last_published()' which does not exist — use get_published()"
        )

    def test_no_wh_stream_iterator(self, sdk_python_doc: str) -> None:
        """Docs must not reference 'wh.stream(Type)' async iterator (does not exist)."""
        assert "wh.stream(" not in sdk_python_doc and ".stream(TextMessage)" not in sdk_python_doc, (
            "SDK docs reference '.stream()' async iterator which does not exist"
        )

    def test_documents_mock_mode(self, sdk_python_doc: str) -> None:
        """SDK docs must document mock mode with connect(mock=True)."""
        assert "mock=True" in sdk_python_doc or "mock = True" in sdk_python_doc, (
            "SDK docs must document mock mode: wheelhouse.connect(mock=True)"
        )

    def test_documents_error_handling(self, sdk_python_doc: str) -> None:
        """SDK docs must document error types."""
        assert "ConnectionError" in sdk_python_doc, "SDK docs must document ConnectionError"
        assert "PublishTimeout" in sdk_python_doc, "SDK docs must document PublishTimeout"

    def test_documents_register_type_decorator(self, sdk_python_doc: str) -> None:
        """SDK docs must show @wheelhouse.register_type or @register_type decorator."""
        assert "register_type" in sdk_python_doc, "SDK docs must document register_type"

    def test_correct_mock_connection_constructor(self, sdk_python_doc: str) -> None:
        """MockConnection takes no constructor args — docs must not show MockConnection("main")."""
        assert 'MockConnection("' not in sdk_python_doc, (
            "SDK docs show MockConnection with constructor arg — it takes no args"
        )


class TestCustomSurfaceGuideAccuracy:
    """Verify docs/src/content/docs/guides/custom-surface.md matches actual SDK API."""

    @pytest.fixture
    def guide_doc(self) -> str:
        path = PROJECT_ROOT / "docs" / "src" / "content" / "docs" / "guides" / "custom-surface.md"
        assert path.exists(), f"Custom surface guide not found at {path}"
        return path.read_text()

    def test_no_base_stream_object(self, guide_doc: str) -> None:
        """Guide must not reference BaseStreamObject."""
        assert "BaseStreamObject" not in guide_doc

    def test_no_last_published(self, guide_doc: str) -> None:
        """Guide must not reference last_published()."""
        assert "last_published" not in guide_doc

    def test_no_mock_connection_with_arg(self, guide_doc: str) -> None:
        """MockConnection takes no constructor args."""
        assert 'MockConnection("' not in guide_doc

    def test_uses_async_api(self, guide_doc: str) -> None:
        """Guide code samples should use async/await pattern."""
        assert "await" in guide_doc or "async" in guide_doc, (
            "Custom surface guide must use async/await pattern"
        )
