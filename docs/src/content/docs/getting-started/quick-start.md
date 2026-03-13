---
title: Quick Start
description: Deploy your first agent in under 5 minutes
---

This guide gets you from zero to a running agent in under 5 minutes.

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

Secrets are stored outside git, never in `.wh` files.

## 2. Write your first `.wh` file

Create `topology.wh`:

```yaml
api_version: wheelhouse.dev/v1
name: my-first-topology

# Streams are the typed message bus connecting all components.
# retention: how long objects are kept (omit to keep forever)
streams:
  - name: main
    retention: "30d"

# Agents subscribe to streams, decide, and publish back.
# guardrails.max_replicas caps autonomous scaling topology-wide.
agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main]

guardrails:
  max_replicas: 2    # prevents unconstrained autonomous scaling
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

`+` create · `~` update · `!` destroy. Preview is always shown before apply.

## 4. Apply

```sh
wh deploy apply topology.wh
```

```
  Applying...

  ✓ stream main    created
  ✓ agent donna    started

  2 created · 0 changed · 0 destroyed  [state: a3f9c2]
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
[2026-03-12T10:00:01Z] [TextMessage] [donna] Hello — I'm ready.
```

Your agent is running. See [Deploy your first agent](/guides/first-agent) for the complete walkthrough including Telegram setup.
