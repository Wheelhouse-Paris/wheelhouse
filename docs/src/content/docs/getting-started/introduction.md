---
title: Introduction
description: What is Wheelhouse and why does it exist?
hero:
  title: Write a plan. Watch your agents come alive.
  tagline: Declare your agent topology in a `.wh` file. Apply it. Wheelhouse handles wiring, process management, and restarts.
  actions:
    - text: Quick Start →
      link: /getting-started/quick-start/
      variant: primary
    - text: View on GitHub
      link: https://github.com/Wheelhouse-Paris/wheelhouse
      variant: secondary
---

Wheelhouse is the operating infrastructure for autonomous agent factories.

It lets you **specify, deploy, monitor, and let your agents operate their own infrastructure** — without human intervention.

## The core idea

The agent is the operator.

Unlike existing orchestrators (LangGraph, CrewAI, AutoGen) — frameworks written by humans for humans — Wheelhouse is infrastructure operated by agents themselves. The `.wh` file is not static config — it is a living topology that agents read, modify, and apply as their needs evolve.

## Key primitives

| Primitive | Role |
|-----------|------|
| **Stream** | Real-time typed object bus — the connective tissue |
| **Agent** | Autonomous subscriber/publisher with a `observe → decide → act` cycle |
| **Surface** | Bridge between human users and a stream (Telegram, CLI, custom) |
| **Skill** | Versioned recipe stored in git, invoked via stream object |
| **Cron** | First-class scheduler that publishes `CronEvent` into streams |
| **`.wh` file** | Declarative topology declaration — the Dockerfile of agentic infrastructure |

## Why Wheelhouse?

Three things that don't exist anywhere else:

1. **LLM-native IaC** — `.wh` files are designed to be read and written by language models
2. **Versioned communication contracts** — every stream message has a typed, versioned Protobuf schema
3. **Autonomous observation→decision→action loop** — the stream is the observation, compaction is the analysis, the `.wh` is the intent, apply is the action — all signed and audited in git

## Next steps

- [Install Wheelhouse](/getting-started/installation)
- [Quick Start — your first agent in 5 minutes](/getting-started/quick-start)
