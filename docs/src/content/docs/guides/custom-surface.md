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

async def main():
    async with wheelhouse.Surface("my-surface") as surface:
        await surface.connect("main")

        # Publish a message
        await surface.publish(wheelhouse.types.TextMessage(
            content="Hello from my surface"
        ))

        # Listen for responses
        async for obj in surface.subscribe():
            print(f"Received: {obj}")

asyncio.run(main())
```

## Register a custom type

```python
@wheelhouse.register_type("myapp.CustomEvent")
class CustomEvent(wheelhouse.BaseStreamObject):
    event_type: str
    payload: dict
```

Use a namespace (e.g. `myapp.`) to avoid collisions with other surfaces.

## Declare in your .wh file

```yaml
surfaces:
  - name: my-surface
    type: custom
    image: my-org/my-surface:latest
    streams: [main]
```

## Test without a running topology

```python
from wheelhouse.testing import MockConnection

wh = MockConnection("main")
wh.publish(TextMessage(text="Hello from my surface"))
received = wh.last_published()
```
