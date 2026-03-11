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
from wheelhouse import connect, TextMessage

wh = connect("main")

@wh.subscribe(TextMessage)
def on_message(msg):
    print(msg.text)

wh.run()
```

If Wheelhouse is not running, `connect()` fails immediately with an actionable message:

```
WheelhouseNotRunning: No topology running at localhost:5555.
Deploy one first: wh deploy apply my-topology.wh
See ERRORS.md#WH-001 for details.
```

For non-standard deployments (e.g. inside a container), pass the endpoint explicitly or set `WH_URL`:

```python
wh = connect("main", endpoint="tcp://host.docker.internal:5555")
# or: export WH_URL=tcp://host.docker.internal:5555
```

## Publish an object

```python
from wheelhouse import connect, TextMessage

wh = connect("main")
wh.publish(TextMessage(text="Hello from Python"))
```

## Register a custom type

Custom types are Python dataclasses. Field values must be JSON-safe (`str`, `int`, `float`, `bool`, `None`, `list`, `dict[str, ...]`).

```python
from wheelhouse import connect, BaseStreamObject, register_type

@register_type("biotech.MoleculeObject")
class MoleculeObject(BaseStreamObject):
    smiles: str
    name: str
    metadata: dict[str, str]   # string keys required

wh = connect("main")
wh.publish(MoleculeObject(
    smiles="CC(=O)Oc1ccccc1C(=O)O",
    name="Aspirin",
    metadata={"source": "user-input"}
))
```

Type namespacing prevents collisions. The `wheelhouse.*` namespace is reserved for core types.

## Async API

For advanced use cases (web servers, async frameworks):

```python
from wheelhouse import async_connect, TextMessage

async def main():
    async with async_connect("main") as wh:
        async for msg in wh.stream(TextMessage):
            print(msg.text)
```

## Testing

Import from `wheelhouse.testing` — never use production imports in test code:

```python
from wheelhouse.testing import MockConnection
from wheelhouse import TextMessage

def test_my_handler():
    wh = MockConnection("main")
    wh.publish(TextMessage(text="hello"))
    assert wh.last_published().text == "hello"
```

`MockConnection` verifies API usage and message structure. It does not simulate delivery ordering or network conditions — use integration tests with a real topology for those.

## Core types

All core types are importable from `wheelhouse`:

```python
from wheelhouse import (
    TextMessage,
    FileMessage,
    Reaction,
    SkillInvocation,
    SkillResult,
    SkillProgress,
    CronEvent,
)
```

`SkillProgress` is published by agents during long-running skill execution to signal progress to surfaces.
