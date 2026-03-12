# Wheelhouse Error Codes

Every error produced by the `wh` CLI includes a numeric code in the format `WH-XXXX`.
These codes are **stable** — once published, a code will not be reassigned or removed.

Use `--format json` to get machine-parseable error output with the code as a numeric field.

## Error Code Table

| Code | Name | Description | Fix Instruction |
|------|------|-------------|-----------------|
| WH-1001 | CONNECTION_ERROR | Cannot connect to Wheelhouse. The local Wheelhouse instance is not running or not reachable. | Run `wh deploy apply` to start Wheelhouse, or check that the topology file exists and is valid. |
| WH-2001 | LINT_ERROR | Topology file lint validation failed. A field or value in the `.wh` file is invalid. | Check the `context.file`, `context.line`, and `context.field` in the error output. Fix the indicated field in your topology file. Run `wh deploy lint` to re-validate. |
| WH-2002 | PLAN_ERROR | Deploy plan generation failed. The topology could not be resolved into a valid deployment plan. | Review the error message for details. Ensure all referenced agents, streams, and dependencies exist and are correctly configured. |
| WH-2003 | APPLY_ERROR | Deploy apply failed. The deployment could not be completed. | Check the error message for the specific failure. Common causes: missing container images, insufficient permissions, port conflicts. Run `wh deploy plan` first to preview changes. |
| WH-3001 | STREAM_ERROR | Stream operation failed. The requested stream does not exist or is not accessible. | Verify the stream name with `wh ps`. Ensure the topology includes the stream and that Wheelhouse is running. |
| WH-4001 | CONFIG_ERROR | Configuration error. A configuration file is missing, malformed, or contains invalid values. | Check the indicated file path and field. Ensure YAML syntax is valid and all required fields are present. |
| WH-9001 | INTERNAL_ERROR | An unexpected internal error occurred. This indicates a bug in Wheelhouse. | Please report this error at https://github.com/Wheelhouse-Paris/wheelhouse/issues with the full error output and steps to reproduce. |

## JSON Error Format

When using `--format json`, errors are returned as:

```json
{
  "v": 1,
  "status": "error",
  "code": 1001,
  "error_name": "CONNECTION_ERROR",
  "message": "Wheelhouse not running",
  "context": {
    "file": null,
    "line": null,
    "field": null
  }
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `v` | integer | Envelope version (always `1`) |
| `status` | string | Always `"error"` for error responses |
| `code` | integer | Numeric error code (see table above) |
| `error_name` | string | Symbolic error name (e.g., `"CONNECTION_ERROR"`) |
| `message` | string | Human-readable error description |
| `context.file` | string or null | Source file where error was detected |
| `context.line` | integer or null | Line number in source file |
| `context.field` | string or null | Field name that caused the error |

## Exit Codes

| Exit Code | Meaning |
|-----------|---------|
| 0 | Success |
| 1 | Error (see error output for details) |
| 2 | Plan change detected (`wh deploy plan` only) |
