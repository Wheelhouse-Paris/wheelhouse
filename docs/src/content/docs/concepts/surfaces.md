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

Build your own surface with the Python SDK:

```python
import wheelhouse

@wheelhouse.register_type("biotech.MoleculeObject")
class MoleculeObject(wheelhouse.BaseStreamObject):
    smiles: str
    name: str
    metadata: dict

async def main():
    async with wheelhouse.Surface("biotech-sketcher") as surface:
        await surface.connect("main")
        await surface.publish(MoleculeObject(
            smiles="CC(=O)Oc1ccccc1C(=O)O",
            name="Aspirin",
            metadata={}
        ))
```

## Hot-pluggable

Surfaces can be added to a running topology without restarting existing agents:

```sh
wh deploy plan my-agent.wh  # shows + surface biotech-sketcher
wh deploy apply my-agent.wh
```
