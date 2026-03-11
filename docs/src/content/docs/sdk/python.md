---
title: Python SDK
description: Build custom surfaces and skills with the Wheelhouse Python SDK
---

The Python SDK enables developers to build custom surfaces and interact with streams programmatically.

**Requirements:** Python 3.10+

## Installation

```sh
pip install wheelhouse-sdk
# or
uv add wheelhouse-sdk
```

## Connect to a stream

```python
import wheelhouse

async with wheelhouse.Client() as client:
    stream = await client.stream("main")

    async for obj in stream.subscribe():
        print(obj)
```

## Publish an object

```python
from wheelhouse.types import TextMessage

await stream.publish(TextMessage(content="Hello from Python"))
```

## Register a custom type

```python
@wheelhouse.register_type("biotech.MoleculeObject")
class MoleculeObject(wheelhouse.BaseStreamObject):
    smiles: str
    name: str
    metadata: dict

await stream.publish(MoleculeObject(
    smiles="CC(=O)Oc1ccccc1C(=O)O",
    name="Aspirin",
    metadata={"source": "user-input"}
))
```

## Test mode

Test your surface without a running broker:

```python
async with wheelhouse.MockClient() as client:
    stream = await client.stream("main")
    # Full API available, no broker required
```
