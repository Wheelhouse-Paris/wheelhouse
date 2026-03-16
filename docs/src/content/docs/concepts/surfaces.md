---
title: Surfaces
description: Bridges between human users and streams
---

A **surface** is a UI that allows a human user to exchange objects with a stream. It is the boundary between the human world and the agentic infrastructure.

## Standard surfaces

Shipped with Wheelhouse:

| Surface | Status | Description |
|---------|--------|-------------|
| Telegram | ✅ MVP | Bot-based chat interface, with multi-chat and topic routing |
| CLI | ✅ MVP | Interactive terminal session (`wh surface cli`) |
| WhatsApp | Phase 2 | |

## Declaring surfaces in `.wh`

### Simple surface (single stream)

```yaml
surfaces:
  - name: telegram
    kind: telegram
    stream: main
```

### Multi-chat Telegram surface

When you need different Telegram topics or groups to route to different streams, use the `chats:` block instead of `stream:`:

```yaml
surfaces:
  - name: telegram
    kind: telegram
    chats:
      - id: "My Group"
        threads:
          - id: "General"
            stream: main
          - id: "Research"
            stream: research
          - id: "Admin"
            stream: wh-admin
```

On `wh deploy apply`, the CLI resolves group and thread names to their Telegram IDs and writes a routing file (`<topology_dir>/.wh/telegram-routing.json`) that is passed to the surface process via `WH_TELEGRAM_ROUTING_FILE`.

Secrets (e.g. `TELEGRAM_BOT_TOKEN`) are stored via `wh secrets init` and injected automatically at deploy time — they never go in the topology file.

## Surface lifecycle

Surfaces are native processes managed by the `wh` CLI. Their PID files live in `~/.wh/pids/`.

| Command | Description |
|---------|-------------|
| `wh deploy apply <file>` | Provision all surfaces declared in the topology |
| `wh deploy destroy <file>` | Stop and remove all surfaces |
| `wh surface restart <name>` | Kill and respawn without a full deploy cycle |
| `wh surface stop <name>` | Kill without respawning |

`wh surface restart` and `wh surface stop` must be run from the topology directory (where `.wh/state.json` lives):

```sh
cd ~/my-agents
wh surface restart telegram   # apply a new binary or routing config
wh surface stop telegram      # take the surface offline
```

`restart` re-reads the running process's environment via `ps eww` so secrets and routing config are preserved automatically. If the surface is not running, it performs a fresh start.

## Setting up Telegram

**Step 1** — Create a bot via [@BotFather](https://t.me/BotFather) and copy the token.

**Step 2** — Store the token in the keychain:

```sh
wh secrets init
```

Enter the token when prompted for "Telegram bot token". It is stored in the system keychain and never written to disk or committed to git.

**Step 3** — Add the surface to `topology.wh` (use `chats:` for multi-topic groups):

```yaml
surfaces:
  - name: telegram
    kind: telegram
    chats:
      - id: "My Group"
        threads:
          - id: "General"
            stream: main
```

**Step 4** — Apply:

```sh
wh deploy apply topology.wh
```

The CLI resolves group and topic names to Telegram IDs, writes the routing file, and starts the `wh-telegram` process. Open Telegram and message your bot to verify.

After a binary upgrade, apply the new binary without touching git state:

```sh
sudo make install
cd ~/my-agents
wh surface restart telegram
```

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
