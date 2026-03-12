"""Wheelhouse MCP Server — exposes SDK reference, Protobuf schemas, examples, and type validation.

This MCP server enables Claude Code to generate correct Wheelhouse SDK code
by providing on-demand access to SDK documentation, Protobuf contracts, and
working examples.

Transport: stdio (standard for local MCP servers).
"""

from __future__ import annotations

import os
import re
from pathlib import Path

from mcp.server.fastmcp import FastMCP

# Resolve project root: prefer WH_ROOT env var, fall back to __file__-relative
_PROJECT_ROOT = Path(os.environ.get("WH_ROOT", Path(__file__).parent.parent.parent))

mcp = FastMCP(
    "wheelhouse",
    instructions="Wheelhouse SDK reference, Protobuf schemas, examples, and type validation",
)


def _find_project_root() -> Path:
    """Return the resolved project root directory."""
    root = _PROJECT_ROOT
    if not root.exists():
        raise FileNotFoundError(
            f"Project root not found at {root}. Set WH_ROOT environment variable."
        )
    return root


@mcp.tool()
def get_sdk_reference() -> str:
    """Get the complete Wheelhouse Python SDK API reference.

    Returns a structured overview of all public functions, classes, their
    signatures, and usage patterns. Use this when generating Wheelhouse
    SDK code to ensure correct API usage.
    """
    root = _find_project_root()

    # Read the SDK __init__.py for public API
    init_path = root / "sdk" / "python" / "wheelhouse" / "__init__.py"
    core_path = root / "sdk" / "python" / "wheelhouse" / "_core.py"
    types_path = root / "sdk" / "python" / "wheelhouse" / "types.py"
    errors_path = root / "sdk" / "python" / "wheelhouse" / "errors.py"
    testing_path = root / "sdk" / "python" / "wheelhouse" / "testing.py"
    fixtures_path = root / "sdk" / "python" / "wheelhouse" / "fixtures.py"

    parts = ["# Wheelhouse Python SDK Reference\n"]

    for label, path in [
        ("## Public API (__init__.py)", init_path),
        ("## Core Implementation (_core.py)", core_path),
        ("## Types (types.py)", types_path),
        ("## Errors (errors.py)", errors_path),
        ("## Testing (testing.py)", testing_path),
        ("## Fixtures (fixtures.py)", fixtures_path),
    ]:
        parts.append(f"\n{label}\n")
        if path.exists():
            parts.append(f"```python\n{path.read_text()}\n```\n")
        else:
            parts.append("*File not found*\n")

    return "\n".join(parts)


@mcp.tool()
def get_protobuf_schemas(type_name: str | None = None) -> str:
    """Get Wheelhouse Protobuf schema definitions.

    Args:
        type_name: Optional filter. If provided, returns only the proto file
                   containing this type (e.g. "TextMessage", "SkillInvocation").
                   If None, returns all proto files.

    Returns:
        Proto file contents with file paths as headers.
    """
    root = _find_project_root()
    proto_dir = root / "proto" / "wheelhouse" / "v1"

    if not proto_dir.exists():
        return f"Proto directory not found at {proto_dir}"

    proto_files = sorted(proto_dir.glob("*.proto"))
    if not proto_files:
        return "No .proto files found"

    parts = ["# Wheelhouse Protobuf Schemas\n"]

    for proto_file in proto_files:
        content = proto_file.read_text()

        # If type_name filter is set, only include files containing that type
        if type_name and type_name not in content:
            continue

        rel_path = proto_file.relative_to(root)
        parts.append(f"\n## {rel_path}\n")
        parts.append(f"```protobuf\n{content}\n```\n")

    if len(parts) == 1:
        return f"No proto file found containing type '{type_name}'"

    return "\n".join(parts)


@mcp.tool()
def get_examples(example_number: int | None = None) -> str:
    """Get Wheelhouse SDK example files.

    Args:
        example_number: Optional example number (1-4). If None, returns all examples.
            1: Register a custom type (no connection needed)
            2: Publish and subscribe with core types
            3: Custom type + full surface loop
            4: Testing with mock mode

    Returns:
        Example file contents with descriptions.
    """
    root = _find_project_root()
    examples_dir = root / "examples"

    if not examples_dir.exists():
        return f"Examples directory not found at {examples_dir}"

    example_files = sorted(examples_dir.glob("*.py"))
    if not example_files:
        return "No example files found"

    parts = ["# Wheelhouse SDK Examples\n"]

    for ex_file in example_files:
        # Extract number from filename like "01_register_type.py"
        match = re.match(r"(\d+)_", ex_file.name)
        if not match:
            continue

        num = int(match.group(1))
        if example_number is not None and num != example_number:
            continue

        content = ex_file.read_text()
        # Extract docstring for description
        doc_match = re.search(r'"""(.*?)"""', content, re.DOTALL)
        description = doc_match.group(1).strip().split("\n")[0] if doc_match else ex_file.name

        parts.append(f"\n## Example {num}: {description}\n")
        parts.append(f"**File:** `{ex_file.name}`\n")
        parts.append(f"```python\n{content}\n```\n")

    if len(parts) == 1:
        if example_number is not None:
            return f"Example {example_number} not found"
        return "No examples found"

    return "\n".join(parts)


@mcp.tool()
def validate_type_name(type_name: str) -> str:
    """Validate a Wheelhouse type name for @register_type.

    Checks that the name follows the required <namespace>.<TypeName> format
    and does not use the reserved 'wheelhouse.*' namespace.

    Args:
        type_name: The type name to validate (e.g. "myapp.SensorReading").

    Returns:
        Validation result with details.
    """
    if "." not in type_name:
        return (
            f"INVALID: '{type_name}' must be in format '<namespace>.<TypeName>'. "
            f"Example: 'myapp.{type_name}'"
        )

    dot_pos = type_name.index(".")
    namespace = type_name[:dot_pos]
    short_name = type_name[dot_pos + 1:]

    if not namespace:
        return f"INVALID: '{type_name}' has empty namespace"

    if not short_name:
        return f"INVALID: '{type_name}' has empty type name after dot"

    if "." in short_name:
        return f"INVALID: '{type_name}' must have exactly one dot separator"

    if namespace == "wheelhouse":
        return (
            f"INVALID: Namespace 'wheelhouse' is reserved for core types (ADR-004). "
            f"Use a custom namespace like 'myapp.{short_name}'"
        )

    return f"VALID: namespace='{namespace}', type='{short_name}'"


if __name__ == "__main__":
    mcp.run()
