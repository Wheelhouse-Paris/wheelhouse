---
title: Claude Code MCP
description: Use the Wheelhouse MCP server for AI-assisted surface development
---

The Wheelhouse MCP (Model Context Protocol) server gives Claude Code direct access to SDK documentation, Protobuf schemas, working examples, and type validation. This enables Claude Code to generate correct Wheelhouse SDK code without hallucinating APIs.

## Setup

### Automatic (project-level)

Clone the Wheelhouse repository. The `.mcp.json` file at the project root auto-configures the MCP server for Claude Code.

### Manual (global)

Add to your Claude Code configuration (`~/.config/claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "wheelhouse": {
      "command": "python",
      "args": ["/path/to/wheelhouse/mcp/wheelhouse-mcp/server.py"]
    }
  }
}
```

### Install dependencies

```sh
cd mcp/wheelhouse-mcp
pip install -e .
```

## Available tools

### `get_sdk_reference`

Returns the complete Python SDK source code with all public APIs, classes, and their signatures. Use this when you need Claude Code to generate surface code or understand the SDK API.

**Parameters:** None

### `get_protobuf_schemas`

Returns Wheelhouse Protobuf schema definitions from `proto/wheelhouse/v1/`.

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `type_name` | `str \| None` | Filter to files containing this type name |

**Example types:** `TextMessage`, `FileMessage`, `SkillInvocation`, `SkillResult`, `CronEvent`

### `get_examples`

Returns SDK example files demonstrating common patterns.

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `example_number` | `int \| None` | Specific example (1-4), or all if omitted |

**Available examples:**

1. Register a custom type (no connection needed)
2. Publish and subscribe with core types
3. Custom type + full surface loop
4. Testing with mock mode

### `validate_type_name`

Validates a type name for use with `@wheelhouse.register_type`. Checks format and reserved namespaces.

**Parameters:**

| Name | Type | Description |
|------|------|-------------|
| `type_name` | `str` | The type name to validate (e.g. `myapp.SensorReading`) |

**Rules enforced:**

- Must contain exactly one dot: `<namespace>.<TypeName>`
- Namespace and type name must be non-empty
- `wheelhouse.*` namespace is reserved (ADR-004)

## Usage examples

Once configured, ask Claude Code:

- "Generate a Wheelhouse surface that processes sensor readings"
- "Show me the Protobuf schema for SkillInvocation"
- "Is `analytics.PageView` a valid type name?"
- "Write pytest tests for my custom surface using mock mode"

Claude Code will call the appropriate MCP tools to get accurate information before generating code.
