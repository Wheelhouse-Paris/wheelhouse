---
title: Python SDK
description: Build custom surfaces and interact with Wheelhouse streams using the Python SDK
---

The Python SDK enables developers to build custom surfaces and interact with streams programmatically.

**Requirements:** Python 3.10+

## Installation

```sh
pip install wheelhouse-sdk
# or
uv add wheelhouse-sdk
```

## Connecting

All SDK operations require a connection. The `connect()` function is async and returns a `Connection` object:

```python
import asyncio
import wheelhouse

async def main():
    conn = await wheelhouse.connect()
    # ... use conn ...
    await conn.close()

asyncio.run(main())
```

Use the `async with` pattern for automatic cleanup:

```python
import asyncio
import wheelhouse

async def main():
    async with await wheelhouse.connect() as conn:
        pass  # connection auto-closes when block exits

asyncio.run(main())
```

### Connection options

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `endpoint` | `str \| None` | `None` | Wheelhouse endpoint URL |
| `mock` | `bool` | `False` | Use in-memory mock (no Wheelhouse needed) |

Endpoint resolution priority:

1. Explicit `endpoint=` parameter
2. `WH_URL` environment variable
3. Default: `tcp://127.0.0.1:5555`

```python
# Explicit endpoint
conn = await wheelhouse.connect(endpoint="tcp://192.168.1.10:5555")

# Environment variable
# export WH_URL=tcp://host.docker.internal:5555
conn = await wheelhouse.connect()
```

If Wheelhouse is not running, `connect()` raises `wheelhouse.ConnectionError` with an actionable message.

## Publishing

```python
import asyncio
import wheelhouse
from wheelhouse.types import TextMessage

async def main():
    async with await wheelhouse.connect() as conn:
        await conn.publish("my-stream", TextMessage(content="Hello!"))

asyncio.run(main())
```

For confirmed delivery (waits for WAL acknowledgement):

```python
async with await wheelhouse.connect() as conn:
    try:
        await conn.publish_confirmed("my-stream", msg, timeout=5.0)
    except wheelhouse.PublishTimeout:
        print("Delivery not confirmed within timeout")
```

## Subscribing

Register an async handler function for a stream:

```python
import asyncio
import wheelhouse
from wheelhouse.types import TextMessage

async def main():
    async with await wheelhouse.connect() as conn:
        async def on_message(msg):
            print(f"Received: {msg.content}")

        await conn.subscribe("notifications", on_message)

        # Keep running to receive messages
        await asyncio.sleep(60)

asyncio.run(main())
```

Multiple handlers can be registered on the same stream. Each receives every message.

## Registering custom types

Custom types are Python dataclasses with a namespace prefix. The `@register_type` decorator validates the type at decoration time:

```python
import wheelhouse
from dataclasses import dataclass

@wheelhouse.register_type("myapp.SensorReading")
@dataclass
class SensorReading:
    sensor_id: str = ""
    value: float = 0.0
    unit: str = ""
```

**Namespace rules:**

- Format: `<namespace>.<TypeName>` (exactly one dot)
- The `wheelhouse.*` namespace is reserved for core types (ADR-004)
- Classes must have at least one typed annotation

Invalid registrations raise immediately:

| Error | Cause |
|-------|-------|
| `InvalidTypeNameError` | Missing dot, empty namespace, or multiple dots |
| `ReservedNamespaceError` | Using `wheelhouse.*` namespace |
| `TypeError` | Class has no typed fields |

## Core types

Import from `wheelhouse.types`:

```python
from wheelhouse.types import TextMessage, FileMessage, TypedMessage
```

| Type | Fields | Description |
|------|--------|-------------|
| `TextMessage` | `content`, `user_id`, `stream_name` | Plain text message |
| `FileMessage` | `filename`, `content`, `mime_type`, `user_id` | File/binary payload |
| `TypedMessage` | `type_name`, `data`, `raw_bytes`, `is_known` | Received message wrapper |

`TypedMessage` wraps received messages: if the type is known, `data` contains the deserialized object and `is_known` is `True`. For unknown types, `raw_bytes` contains the raw payload.

**Note:** Protobuf types like `SkillInvocation`, `SkillResult`, `SkillProgress`, and `CronEvent` are defined in `proto/wheelhouse/v1/` but are Rust-side only in the current MVP. They will be exposed in the Python SDK in a future release.

## Building a Surface

The `Surface` base class wraps a `Connection` and provides `publish`, `publish_confirmed`, and `subscribe` methods:

```python
import asyncio
import wheelhouse
from wheelhouse.types import TextMessage

class NotificationSurface(wheelhouse.Surface):
    async def on_message(self, message):
        print(f"Notification: {message}")

    async def send(self, text: str):
        await self.publish("notifications", TextMessage(content=text))

async def main():
    async with await wheelhouse.connect() as conn:
        surface = NotificationSurface(conn)
        await conn.subscribe("notifications", surface.on_message)
        await surface.send("Hello from my surface!")

asyncio.run(main())
```

## Error handling

All SDK errors inherit from `WheelhouseError` and include a `code` attribute referenced in `ERRORS.md`:

```python
import wheelhouse

try:
    conn = await wheelhouse.connect()
except wheelhouse.ConnectionError as e:
    print(f"Error [{e.code}]: {e}")
```

Catch errors by type directly from the `wheelhouse` namespace:

| Exception | Code | When |
|-----------|------|------|
| `wheelhouse.ConnectionError` | `CONNECTION_ERROR` | Wheelhouse not running or unreachable |
| `wheelhouse.PublishTimeout` | `PUBLISH_TIMEOUT` | `publish_confirmed()` timed out |
| `wheelhouse.StreamNotFound` | `STREAM_NOT_FOUND` | Requested stream does not exist |

Additional errors from `wheelhouse.errors`:

| Exception | Code | When |
|-----------|------|------|
| `InvalidTypeNameError` | `INVALID_TYPE_NAME` | Bad format in `@register_type` |
| `ReservedNamespaceError` | `RESERVED_NAMESPACE` | Using `wheelhouse.*` namespace |
| `RegistryFullError` | `REGISTRY_FULL` | Type registry at capacity |

## Testing with mock mode

Use `mock=True` to develop and test without a running Wheelhouse instance or Podman installation:

```python
import asyncio
import wheelhouse
from wheelhouse.types import TextMessage

async def main():
    conn = await wheelhouse.connect(mock=True)

    received = []
    async def on_message(msg):
        received.append(msg)

    await conn.subscribe("test-stream", on_message)
    await conn.publish("test-stream", TextMessage(content="hello"))

    assert len(received) == 1
    assert received[0].content == "hello"
    await conn.close()

asyncio.run(main())
```

In mock mode, published messages are automatically echoed to subscribers in the same session.

### Mock utilities

```python
conn = await wheelhouse.connect(mock=True)

# Inspect published messages
published = conn.get_published("my-stream")  # list of (stream, message) tuples

# Get all messages as TypedMessage objects
messages = conn.get_messages()

# Reset state between test scenarios
conn.reset()
```

### Pytest fixtures

Import from `wheelhouse.fixtures` for convenient test setup:

```python
from wheelhouse.fixtures import mock_connection
from wheelhouse.types import TextMessage

async def test_my_handler(mock_connection):
    received = []

    async def handler(msg):
        received.append(msg)

    await mock_connection.subscribe("stream", handler)
    await mock_connection.publish("stream", TextMessage(content="test"))
    assert len(received) == 1
```

Available fixtures: `mock_connection`, `mock_surface` (identical, use `mock_surface` when wrapping in a `Surface` subclass).

## API reference

### `wheelhouse` module

| Symbol | Type | Description |
|--------|------|-------------|
| `connect(endpoint, mock)` | async function | Connect to Wheelhouse |
| `Surface` | class | Base class for custom surfaces |
| `register_type(name)` | decorator | Register a custom Protobuf type |
| `ConnectionError` | exception | Wheelhouse not reachable |
| `PublishTimeout` | exception | Confirmed publish timed out |
| `StreamNotFound` | exception | Stream does not exist |

### `Connection` methods

| Method | Description |
|--------|-------------|
| `publish(stream, message)` | Fire-and-forget publish |
| `publish_confirmed(stream, message, timeout)` | Publish with WAL ack |
| `subscribe(stream, handler)` | Register async message handler |
| `register_type(type_name, type_class)` | Instance-level type registration |
| `close()` | Close connection and release resources |

### `MockConnection` methods

All `Connection` methods plus:

| Method | Description |
|--------|-------------|
| `get_published(stream)` | Get published messages, optionally filtered |
| `get_messages()` | Get all messages as `TypedMessage` list |
| `simulate_message(stream, message)` | Inject a message to handlers |
| `reset()` | Clear all mock state |
| `clear()` | Clear all mock state (alias) |
