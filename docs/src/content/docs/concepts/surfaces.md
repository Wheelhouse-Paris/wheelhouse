---
title: Surfaces
description: Bridges between human users and streams
---

A **surface** is a UI that allows a human user to exchange objects with a stream. It is the boundary between the human world and the agentic infrastructure.

## Standard surfaces

Shipped with Wheelhouse:

| Surface | Status | Description |
|---------|--------|-------------|
| Telegram | ✅ MVP | Bot-based chat interface |
| CLI | ✅ MVP | Interactive terminal session (`wh surface cli`) |
| WhatsApp | Phase 2 | |

## Declaring surfaces in `.wh`

Surfaces are declared alongside agents and streams. The provisioning layer starts and stops surface containers automatically on `wh deploy apply` / `wh deploy destroy`.

```yaml
surfaces:
  - name: telegram
    kind: telegram
    image: ghcr.io/wheelhouse-paris/wh-telegram:latest
    stream: main
```

The provisioning layer automatically injects these env vars into every surface container:

| Env var | Value |
|---------|-------|
| `WH_URL` | Broker ZMQ endpoint (set by provisioning layer) |
| `WH_SURFACE_NAME` | Value of `name` from the topology |
| `WH_STREAM` | Value of `stream` from the topology |

Secrets (e.g. `TELEGRAM_BOT_TOKEN`) are stored via `wh secrets init` and injected automatically at deploy time — they do not go in the topology file.

## Setting up Telegram

**Step 1** — Create a bot via [@BotFather](https://t.me/BotFather) and copy the token.

**Step 2** — Store the token in the keychain:

```sh
wh secrets init
```

Enter the token when prompted for "Telegram bot token". It is stored in the system keychain and never written to disk or committed to git.

**Step 3** — Add the surface to `topology.wh`:

```yaml
surfaces:
  - name: telegram
    kind: telegram
    image: ghcr.io/wheelhouse-paris/wh-telegram:latest
    stream: main
```

**Step 4** — Apply:

```sh
wh deploy apply topology.wh
```

The `TELEGRAM_BOT_TOKEN` from the keychain is automatically injected into the container. Open Telegram and message your bot to verify.

## The CLI surface

Connect to any stream directly from the terminal — no bot or credentials required:

```sh
wh surface cli --stream main
```

```
Connected to stream 'main'. Type a message and press Enter. Ctrl+C to quit.
> Hello
[donna] Hi! How can I help?
```

The CLI surface uses the same ZMQ PUB/SUB mechanism as all other surfaces. It probes broker liveness before connecting and reconnects automatically on transient failures.

Endpoint configuration (defaults match broker defaults — only set if running non-standard ports):

| Env var | Default | Description |
|---------|---------|-------------|
| `WH_PUB_ENDPOINT` | `tcp://127.0.0.1:{WH_PUB_PORT}` | Broker PUB socket |
| `WH_SUB_ENDPOINT` | `tcp://127.0.0.1:{WH_SUB_PORT}` | Broker SUB socket |
| `WH_CONTROL_ENDPOINT` | `tcp://127.0.0.1:{WH_CONTROL_PORT}` | Broker control socket (liveness probe) |

JSON output for agent-readable consumption:

```sh
wh surface cli --stream main --format json
```

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

    await surface.subscribe("main", surface.on_message)
    await surface.publish("main", TextMessage(content="Hello from custom surface"))
```

`Surface` provides three methods wrapping the underlying connection:

| Method | Description |
|--------|-------------|
| `publish(stream, message)` | Fire-and-forget publish |
| `publish_confirmed(stream, message)` | Publish with WAL acknowledgement |
| `subscribe(stream, handler)` | Register an async handler for incoming objects |

Declare custom surfaces in `.wh` with `kind: custom`:

```yaml
surfaces:
  - name: my-surface
    kind: custom
    image: my-org/my-surface:latest
    stream: main
```

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
