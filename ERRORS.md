# Wheelhouse Error Codes

Every error produced by the `wh` CLI includes a numeric code in the format `WH-XXXX`.
These codes are **stable** — once published, a code will not be reassigned or removed.

Use `--format json` to get machine-parseable error output with the code as a numeric field.

## CLI Error Code Table

| Code | Name | Description | Fix Instruction |
|------|------|-------------|-----------------|
| WH-1001 | CONNECTION_ERROR | Cannot connect to Wheelhouse. The local Wheelhouse instance is not running or not reachable. | Run `wh deploy apply` to start Wheelhouse, or check that the topology file exists and is valid. |
| WH-2001 | LINT_ERROR | Topology file lint validation failed. A field or value in the `.wh` file is invalid. | Check the `context.file`, `context.line`, and `context.field` in the error output. Fix the indicated field in your topology file. Run `wh deploy lint` to re-validate. |
| WH-2002 | PLAN_ERROR | Deploy plan generation failed. The topology could not be resolved into a valid deployment plan. | Review the error message for details. Ensure all referenced agents, streams, and dependencies exist and are correctly configured. |
| WH-2003 | APPLY_ERROR | Deploy apply failed. The deployment could not be completed. | Check the error message for the specific failure. Common causes: missing container images, insufficient permissions, port conflicts. Run `wh deploy plan` first to preview changes. |
| WH-3001 | STREAM_ERROR | Stream operation failed. The requested stream does not exist or is not accessible. | Verify the stream name with `wh ps`. Ensure the topology includes the stream and that Wheelhouse is running. |
| WH-4001 | CONFIG_ERROR | Configuration error. A configuration file is missing, malformed, or contains invalid values. | Check the indicated file path and field. Ensure YAML syntax is valid and all required fields are present. |
| WH-4002 | SECRET_NOT_FOUND | A required secret is not configured. Neither an environment variable nor an OS keychain entry was found for the requested credential. | Run `wh secrets init` to configure credentials. Alternatively, set the corresponding environment variable (e.g., `ANTHROPIC_API_KEY`). |
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

---

## SDK Error Reference (Python)

This section lists Python SDK error codes, their exception classes, and resolution guidance.

### CONNECTION_ERROR

| Field | Value |
|-------|-------|
| Exception | `wheelhouse.errors.ConnectionError` |
| Code | `CONNECTION_ERROR` |
| Description | Wheelhouse is not running or not reachable at the configured endpoint. |
| Common Cause | Wheelhouse process is not started, or `WH_URL` points to an incorrect address. |
| Resolution | Start Wheelhouse, verify the endpoint with `WH_URL` env var or `endpoint=` parameter. |

### NOT_CONNECTED

| Field | Value |
|-------|-------|
| Exception | `wheelhouse.errors.ConnectionError` |
| Code | `NOT_CONNECTED` |
| Description | Attempted to publish or subscribe without an active connection. |
| Common Cause | Calling `publish()` or `subscribe()` before `connect()`, or after `close()`. |
| Resolution | Call `await wheelhouse.connect()` first and use the returned connection. |

### PUBLISH_TIMEOUT

| Field | Value |
|-------|-------|
| Exception | `wheelhouse.errors.PublishTimeout` |
| Code | `PUBLISH_TIMEOUT` |
| Description | `publish_confirmed()` did not receive acknowledgement within the timeout period. |
| Common Cause | Wheelhouse is overloaded, network partition, or stream does not exist. |
| Resolution | Increase the `timeout` parameter, check stream exists, or use fire-and-forget `publish()`. |

### STREAM_NOT_FOUND

| Field | Value |
|-------|-------|
| Exception | `wheelhouse.errors.StreamNotFound` |
| Code | `STREAM_NOT_FOUND` |
| Description | The requested stream does not exist. |
| Common Cause | Typo in stream name, or the stream has not been created yet. |
| Resolution | Verify the stream name, create the stream if needed. |

### RESERVED_NAMESPACE

| Field | Value |
|-------|-------|
| Exception | `wheelhouse.errors.ReservedNamespaceError` |
| Code | `RESERVED_NAMESPACE` |
| Description | Attempted to register a type under the reserved `wheelhouse.*` namespace (ADR-004). |
| Common Cause | Using `@wheelhouse.register_type("wheelhouse.MyType")`. |
| Resolution | Use a custom namespace: `@wheelhouse.register_type("myapp.MyType")`. |

### INVALID_TYPE_NAME

| Field | Value |
|-------|-------|
| Exception | `wheelhouse.errors.InvalidTypeNameError` |
| Code | `INVALID_TYPE_NAME` |
| Description | Type name does not match the required `<namespace>.<TypeName>` format. |
| Common Cause | Missing dot separator, empty namespace, or multiple dots. |
| Resolution | Use exactly one dot: `"namespace.TypeName"`. |

### REGISTRY_FULL

| Field | Value |
|-------|-------|
| Exception | `wheelhouse.errors.RegistryFullError` |
| Code | `REGISTRY_FULL` |
| Description | The type registry has reached its capacity limit (RT-05: 100 per namespace, 10,000 total). |
| Common Cause | Too many custom types registered in a single namespace. |
| Resolution | Remove unused types or increase limits in `wh-policy.yaml`. |

## Catching SDK Errors

```python
import wheelhouse
from wheelhouse.errors import PublishTimeout, ConnectionError

try:
    conn = await wheelhouse.connect()
    await conn.publish_confirmed("stream", message, timeout=5.0)
except wheelhouse.PublishTimeout as e:
    print(f"Error [{e.code}]: {e}")
except wheelhouse.ConnectionError as e:
    print(f"Error [{e.code}]: {e}")
```

All exceptions inherit from `wheelhouse.errors.WheelhouseError` and include:
- `.code` — machine-readable error code (matches this document)
- `str(error)` — human-readable error message
