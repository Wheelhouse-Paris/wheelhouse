"""Acceptance tests for Wheelhouse MCP server (Story 6-4, AC #1, #2).

These tests verify that the MCP server exposes tools for SDK reference,
protobuf schemas, examples, and type name validation.
"""

import json
import sys
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).parent.parent.parent
MCP_SERVER_PATH = PROJECT_ROOT / "mcp" / "wheelhouse-mcp" / "server.py"
MCP_PROJECT_PATH = PROJECT_ROOT / "mcp" / "wheelhouse-mcp"


class TestMcpServerExists:
    """Verify MCP server files exist."""

    def test_server_file_exists(self) -> None:
        """MCP server.py must exist at mcp/wheelhouse-mcp/server.py."""
        assert MCP_SERVER_PATH.exists(), (
            f"MCP server not found at {MCP_SERVER_PATH}"
        )

    def test_pyproject_exists(self) -> None:
        """MCP pyproject.toml must exist."""
        path = MCP_PROJECT_PATH / "pyproject.toml"
        assert path.exists(), f"MCP pyproject.toml not found at {path}"

    def test_readme_exists(self) -> None:
        """MCP README.md must exist."""
        path = MCP_PROJECT_PATH / "README.md"
        assert path.exists(), f"MCP README not found at {path}"


class TestMcpConfiguration:
    """Verify MCP auto-discovery configuration."""

    def test_mcp_json_exists(self) -> None:
        """.mcp.json must exist at project root for Claude Code auto-discovery."""
        path = PROJECT_ROOT / ".mcp.json"
        assert path.exists(), f".mcp.json not found at {path}"

    def test_mcp_json_valid(self) -> None:
        """.mcp.json must be valid JSON with mcpServers key."""
        path = PROJECT_ROOT / ".mcp.json"
        content = path.read_text()
        data = json.loads(content)
        assert "mcpServers" in data, ".mcp.json must have 'mcpServers' key"
        assert "wheelhouse" in data["mcpServers"], ".mcp.json must have 'wheelhouse' server entry"


class TestMcpServerModule:
    """Verify MCP server can be imported and has required tools.

    These tests skip if the 'mcp' package is not installed, since it's
    an optional dependency only needed to run the MCP server.
    """

    @pytest.fixture
    def server_module(self, monkeypatch):
        """Import the MCP server module."""
        try:
            import mcp  # noqa: F401
        except ImportError:
            pytest.skip("mcp package not installed")

        if not MCP_SERVER_PATH.exists():
            pytest.skip("MCP server not yet created")

        # Set WH_ROOT so the server can find the project (cleaned up by monkeypatch)
        monkeypatch.setenv("WH_ROOT", str(PROJECT_ROOT))

        sys.path.insert(0, str(MCP_PROJECT_PATH))
        try:
            import importlib.util
            spec = importlib.util.spec_from_file_location("server", MCP_SERVER_PATH)
            mod = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(mod)
            return mod
        except ImportError as e:
            if "mcp" in str(e):
                pytest.skip("mcp package not installed")
            pytest.fail(f"Cannot import MCP server: {e}")
        finally:
            if sys.path[0] == str(MCP_PROJECT_PATH):
                sys.path.pop(0)

    def test_server_imports(self, server_module) -> None:
        """MCP server module must import successfully."""
        assert server_module is not None

    def test_has_get_sdk_reference(self, server_module) -> None:
        """MCP server must have a get_sdk_reference function."""
        assert hasattr(server_module, "get_sdk_reference"), (
            "MCP server must have a get_sdk_reference tool"
        )

    def test_has_get_protobuf_schemas(self, server_module) -> None:
        """MCP server must have a get_protobuf_schemas function."""
        assert hasattr(server_module, "get_protobuf_schemas"), (
            "MCP server must have a get_protobuf_schemas tool"
        )

    def test_has_get_examples(self, server_module) -> None:
        """MCP server must have a get_examples function."""
        assert hasattr(server_module, "get_examples"), (
            "MCP server must have a get_examples tool"
        )

    def test_has_validate_type_name(self, server_module) -> None:
        """MCP server must have a validate_type_name function."""
        assert hasattr(server_module, "validate_type_name"), (
            "MCP server must have a validate_type_name tool"
        )

    def test_get_sdk_reference_content(self, server_module) -> None:
        """get_sdk_reference must return SDK API information."""
        result = server_module.get_sdk_reference()
        assert "connect" in result, "SDK reference must mention connect()"
        assert "publish" in result, "SDK reference must mention publish()"
        assert "subscribe" in result, "SDK reference must mention subscribe()"
        assert "register_type" in result, "SDK reference must mention register_type()"
        assert "MockConnection" in result, "SDK reference must mention MockConnection"

    def test_get_protobuf_schemas_content(self, server_module) -> None:
        """get_protobuf_schemas must return valid proto content."""
        result = server_module.get_protobuf_schemas()
        assert "TextMessage" in result, "Proto schemas must contain TextMessage"
        assert "syntax" in result, "Proto schemas must contain proto syntax declaration"

    def test_get_protobuf_schemas_filter(self, server_module) -> None:
        """get_protobuf_schemas with type_name filter returns matching file only."""
        result = server_module.get_protobuf_schemas(type_name="SkillInvocation")
        assert "SkillInvocation" in result
        assert "skills.proto" in result

    def test_validate_type_name_valid(self, server_module) -> None:
        """validate_type_name accepts valid names."""
        result = server_module.validate_type_name("myapp.MyType")
        assert "VALID" in result

    def test_validate_type_name_reserved(self, server_module) -> None:
        """validate_type_name rejects wheelhouse.* namespace."""
        result = server_module.validate_type_name("wheelhouse.Foo")
        assert "INVALID" in result
        assert "reserved" in result.lower()

    def test_validate_type_name_no_namespace(self, server_module) -> None:
        """validate_type_name rejects names without namespace."""
        result = server_module.validate_type_name("nonamespace")
        assert "INVALID" in result

    def test_get_examples_all(self, server_module) -> None:
        """get_examples returns all 4 examples."""
        result = server_module.get_examples()
        assert "Example 1" in result
        assert "Example 2" in result
        assert "Example 3" in result
        assert "Example 4" in result

    def test_get_examples_single(self, server_module) -> None:
        """get_examples with number returns specific example."""
        result = server_module.get_examples(example_number=1)
        assert "Example 1" in result
        assert "register_type" in result


class TestMcpDocumentation:
    """Verify MCP documentation page exists."""

    def test_mcp_doc_page_exists(self) -> None:
        """MCP documentation must exist at docs/src/content/docs/sdk/mcp.md."""
        path = PROJECT_ROOT / "docs" / "src" / "content" / "docs" / "sdk" / "mcp.md"
        assert path.exists(), f"MCP doc page not found at {path}"

    def test_sidebar_includes_mcp(self) -> None:
        """Astro sidebar config must include MCP page."""
        config_path = PROJECT_ROOT / "docs" / "astro.config.mjs"
        content = config_path.read_text()
        assert "mcp" in content.lower(), (
            "astro.config.mjs sidebar must include MCP page"
        )
