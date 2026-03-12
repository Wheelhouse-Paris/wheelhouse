# Wheelhouse MCP Server

A [Model Context Protocol](https://modelcontextprotocol.io/) server that gives Claude Code access to the Wheelhouse Python SDK reference, Protobuf schemas, working examples, and type name validation.

## Setup for Claude Code

### Option 1: Project-level (recommended)

The `.mcp.json` file at the project root auto-configures the server. Clone the repo and Claude Code picks it up automatically.

### Option 2: Global configuration

Add to `~/.config/claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "wheelhouse": {
      "command": "python",
      "args": ["<path-to-repo>/mcp/wheelhouse-mcp/server.py"],
      "env": {
        "WH_ROOT": "<path-to-repo>"
      }
    }
  }
}
```

## Install dependencies

```sh
cd mcp/wheelhouse-mcp
pip install -e .
```

## Available tools

| Tool | Description |
|------|-------------|
| `get_sdk_reference` | Complete Python SDK API reference (all source files) |
| `get_protobuf_schemas` | Protobuf schema definitions, with optional type filter |
| `get_examples` | SDK example files (1-4), with optional number filter |
| `validate_type_name` | Validate `@register_type` name format and namespace |

## Usage in Claude Code

Once configured, ask Claude Code questions like:

- "Generate a custom surface that listens for SensorReading events"
- "Show me the Protobuf schema for SkillInvocation"
- "Is `myapp.DataPoint` a valid type name for @register_type?"

Claude Code will automatically call the appropriate MCP tools to get accurate, up-to-date information.
