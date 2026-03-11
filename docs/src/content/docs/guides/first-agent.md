---
title: Deploy your first agent
description: Step-by-step guide to deploying a working agent
---

This guide walks through deploying a complete agent setup with a Telegram surface.

## Prerequisites

- Wheelhouse installed ([Installation](/getting-started/installation))
- Podman running
- Claude API key
- Telegram bot token

## Step 1 — Initialize secrets

```sh
wh secrets init
```

Follow the prompts to store your Claude API key and Telegram bot token securely.

## Step 2 — Create your topology

`my-first-agent.wh`:

```yaml
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local
    retention:
      max_age: 30d

agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main]
    max_replicas: 1

surfaces:
  - name: telegram
    type: telegram
    streams: [main]

cron:
  - name: daily-compaction
    schedule: "0 3 * * *"
    target: main
    action: compact
```

## Step 3 — Plan and apply

```sh
wh deploy plan my-first-agent.wh
```

Review the plan, then apply:

```sh
wh deploy apply my-first-agent.wh
```

## Step 4 — Verify

```sh
wh ps
```

```
NAME    STATUS   PROVIDER   STREAM   REPLICAS   LAST_COMMIT
donna   running  podman     main     1/1        init
```

Send a message to your Telegram bot. Your agent responds.

## Step 5 — Watch the stream

```sh
wh stream tail main
```

You'll see the objects flowing in real time as you interact with the agent.
