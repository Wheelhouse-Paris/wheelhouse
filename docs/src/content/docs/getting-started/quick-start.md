---
title: Quick Start
description: Deploy your first agent in under 5 minutes
---

This guide gets you from zero to a running agent connected to Telegram in under 5 minutes (excluding third-party API key setup).

## 1. Initialize secrets

```sh
wh secrets init
```

Wheelhouse detects available providers and guides you through credential setup. Secrets are stored outside git, never in `.wh` files.

## 2. Write your first `.wh` file

Create `my-agent.wh`:

```yaml
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: main
    provider: local

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

## 3. Preview and apply

```sh
wh deploy plan my-agent.wh
wh deploy apply my-agent.wh
```

## 4. Verify

```sh
wh ps
wh stream tail main
```

Your agent is now running. Send a message in Telegram — it will respond.
