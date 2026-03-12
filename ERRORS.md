# Wheelhouse Error Reference

This document lists all SDK error codes, their exception classes, and resolution guidance.

## Error Codes

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

## Catching Errors

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
