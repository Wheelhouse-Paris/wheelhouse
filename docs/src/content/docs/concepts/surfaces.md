---
title: Surfaces
description: Bridges between human users and streams
---

A **surface** is a UI that allows a human user to exchange objects with a stream. It is the boundary between the human world and the agentic infrastructure.

## Standard surfaces

Shipped with Wheelhouse:

| Surface | Status |
|---------|--------|
| Telegram | ✅ MVP |
| CLI | ✅ MVP |
| WhatsApp | Phase 2 |

## Custom surfaces

Build your own surface with the Python SDK by subclassing `wheelhouse.Surface`:

```python
import wheelhouse
from wheelhouse.types import TextMessage

class MyCLISurface(wheelhouse.Surface):
    async def on_message(self, message):
        print(message.content)

async def main():
    conn = await wheelhouse.connect()
    surface = MyCLISurface(conn)

    # Subscribe a handler to receive messages
    await surface.subscribe("main", surface.on_message)

    # Publish a message to the stream
    await surface.publish("main", TextMessage(content="Hello from custom surface"))
```

`Surface` provides three methods wrapping the underlying connection:

| Method | Description |
|--------|-------------|
| `publish(stream, message)` | Fire-and-forget publish |
| `publish_confirmed(stream, message)` | Publish with WAL acknowledgement |
| `subscribe(stream, handler)` | Register an async handler for incoming objects |

## Custom types

Register application-specific types with `@wheelhouse.register_type`:

```python
import betterproto
import wheelhouse

@wheelhouse.register_type("biotech.MoleculeObject")
class MoleculeObject(betterproto.Message):
    smiles: str = betterproto.string_field(1)
    name: str = betterproto.string_field(2)

conn = await wheelhouse.connect()
surface = wheelhouse.Surface(conn)
await surface.publish("main", MoleculeObject(smiles="CC(=O)Oc1ccccc1C(=O)O", name="Aspirin"))
```
