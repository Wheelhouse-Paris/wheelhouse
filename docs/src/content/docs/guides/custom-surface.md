---
title: Build a custom surface
description: Create a custom UI connected to a Wheelhouse stream
---

This guide shows how to build a custom surface using the Python SDK.

## What is a surface?

A surface is any UI that exchanges typed objects with a stream. Standard surfaces (Telegram, CLI) ship with Wheelhouse. Custom surfaces are built with the SDK.

## Install the SDK

```sh
pip install wheelhouse-sdk
```

## Basic surface

```python
import asyncio
import wheelhouse
from wheelhouse.types import TextMessage

class MyCLISurface(wheelhouse.Surface):
    async def on_message(self, message):
        print(f"Received: {message}")

async def main():
    async with await wheelhouse.connect() as conn:
        surface = MyCLISurface(conn)

        await conn.subscribe("main", surface.on_message)

        await surface.publish("main", TextMessage(
            content="Hello from my surface"
        ))

        await asyncio.sleep(30)

asyncio.run(main())
```

## Register a custom type

```python
import wheelhouse
from dataclasses import dataclass

@wheelhouse.register_type("myapp.CustomEvent")
@dataclass
class CustomEvent:
    event_type: str = ""
    payload: str = ""
```

Use a namespace (e.g. `myapp.`) to avoid collisions with other surfaces. The `wheelhouse.*` namespace is reserved for core types.

## Publish and subscribe with custom types

```python
import asyncio
import wheelhouse
from dataclasses import dataclass

@wheelhouse.register_type("myapp.Query")
@dataclass
class Query:
    question: str = ""
    user_id: str = ""

async def main():
    async with await wheelhouse.connect() as conn:
        async def on_reply(msg):
            print(f"Reply: {msg}")

        await conn.subscribe("assistant", on_reply)

        await conn.publish("assistant", Query(
            question="What is Wheelhouse?",
            user_id="demo"
        ))

asyncio.run(main())
```

## Declare in your .wh file

```yaml
surfaces:
  - name: my-surface
    type: custom
    image: my-org/my-surface:latest
    streams: [main]
```

## Test without a running topology

Use mock mode to develop and test without Wheelhouse or Podman:

```python
import asyncio
import wheelhouse
from wheelhouse.types import TextMessage

async def main():
    conn = await wheelhouse.connect(mock=True)

    received = []

    async def handler(msg):
        received.append(msg)

    await conn.subscribe("main", handler)
    await conn.publish("main", TextMessage(content="Hello from my surface"))

    assert len(received) == 1

    # Inspect what was published
    published = conn.get_published("main")
    assert len(published) == 1

    await conn.close()

asyncio.run(main())
```

For pytest, use the built-in fixtures:

```python
from wheelhouse.fixtures import mock_connection
from wheelhouse.types import TextMessage

async def test_my_surface(mock_connection):
    received = []

    async def handler(msg):
        received.append(msg)

    await mock_connection.subscribe("main", handler)
    await mock_connection.publish("main", TextMessage(content="test"))
    assert len(received) == 1
```
