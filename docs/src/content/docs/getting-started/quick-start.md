---
title: Quick Start
description: Deploy your first agent in under 5 minutes
---

This guide gets you from zero to a running agent in under 5 minutes.

## 0. Create a project directory

Wheelhouse uses git to track topology state. Start from a git repository:

```sh
mkdir my-agents && cd my-agents
git init
```

## 1. Initialize secrets

```sh
wh secrets init
```

Wheelhouse auto-detects available providers and guides you through credential setup:

```
  Detecting available providers...

  ✓ Podman found (v4.9.3)
  ✓ Git configured (you@example.com)
  ? Claude API key  ········ (required for agents)
  ? Telegram bot token  ········ (optional — skip with Enter)

  Secrets stored in macOS Keychain.
  Run 'wh deploy apply topology.wh' to start.
```

Secrets are stored outside git, never in `.wh` files. If you enter a Telegram bot token here, it is automatically injected into any Telegram surface container at deploy time.

## 2. Write your first `.wh` file

Create `topology.wh`:

```yaml
api_version: wheelhouse.dev/v1
name: my-first-topology

streams:
  - name: main
    retention: "30d"

agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main]

guardrails:
  max_replicas: 2
```

## 3. Preview changes

```sh
wh deploy plan topology.wh
```

```
  Changes to apply:

  + agent donna          (new)   podman · agent-claude:latest
  + stream main          (new)   local · retention 30d

  2 to create · 0 to update · 0 to destroy

  Run 'wh deploy apply topology.wh' to apply these changes.
```

## 4. Apply

```sh
wh deploy apply topology.wh
```

```
  Applying...

  1 created · 0 changed · 0 destroyed · 1 streams  [state: a3f9c2]
```

## 5. Verify

```sh
wh ps
```

```
  NAME    STATUS    STREAM    PROVIDER    UPTIME
  ──────────────────────────────────────────────
  donna   running   main      podman      0m

  1 agent  ·  1 running
```

```sh
wh stream tail main
```

```
Tailing stream 'main' — press Ctrl+C to stop
```

## 6. Add a Telegram surface (optional)

If you entered a Telegram bot token in step 1, add a surface to `topology.wh`:

```yaml
api_version: wheelhouse.dev/v1
name: my-first-topology

streams:
  - name: main
    retention: "30d"

agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main]

surfaces:
  - name: telegram
    kind: telegram
    image: ghcr.io/wheelhouse-paris/wh-telegram:latest
    stream: main

guardrails:
  max_replicas: 2
```

Preview and apply:

```sh
wh deploy plan topology.wh
wh deploy apply topology.wh
```

```
  + surface telegram     (new)   podman · wh-telegram:latest

  1 to create · 0 to update · 0 to destroy
```

The provisioning layer automatically injects `WH_URL`, `WH_STREAM`, `WH_SURFACE_NAME`, and your bot token into the container — no manual env config required.

Verify the surface is running:

```sh
wh ps
```

```
  NAME              STATUS    STREAM    PROVIDER    UPTIME
  ────────────────────────────────────────────────────────
  donna             running   main      podman      2m
  surface/telegram  running   main      podman      0m

  1 agent · 1 surface · all running
```

Open Telegram, message your bot, and watch responses appear on the stream:

```sh
wh stream tail main
```

```
[2026-03-14T11:00:01Z] [TextMessage] [telegram/user-42] Hello!
[2026-03-14T11:00:03Z] [TextMessage] [donna]            Hi! How can I help you?
```

## 7. Interact from the CLI surface

You can also talk to any agent directly from the terminal without a bot:

```sh
wh surface cli --stream main
```

```
Connected to stream 'main'. Type a message and press Enter. Ctrl+C to quit.
> Hello from the terminal
[donna] Hi! How can I help you?
```

See [Deploy your first agent](/guides/first-agent) for the complete walkthrough.
