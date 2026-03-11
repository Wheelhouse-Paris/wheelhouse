---
title: Personas
description: The bootloader of an agent — identity, context, and memory in git
---

A **persona** is a set of markdown files versioned in git that define an agent's identity, context, and working memory. Personas are loaded at agent startup, before any stream connection — they are the first thing an agent reads, shaping all subsequent behavior.

## Persona files

| File | Role | Mutable? |
|------|------|----------|
| `SOUL.md` | Immutable identity — purpose, values, core behavioral principles | No |
| `IDENTITY.md` | Context and background — who the agent is, what it represents, what it knows | No |
| `MEMORY.md` | Working memory — decisions, learnings, ongoing context persisted across restarts | Yes — agent updates this |

```
agents/donna/
  SOUL.md
  IDENTITY.md
  MEMORY.md
```

## The bootloader metaphor

The persona is to an agent what a bootloader is to an OS: it runs first, sets the context, and defines the environment in which everything else executes. An agent without a persona has no identity — it would not know what role to play, what values to apply, or what it has learned.

## Startup sequence

```
git pull persona files → load SOUL.md → load IDENTITY.md →
load MEMORY.md → connect to streams → begin observe → decide → act loop
```

## Living memory

`MEMORY.md` is the only persona file an agent is allowed to modify autonomously. On each significant operation, the agent commits an update:

```
[donna] memory: updated task context after researcher scale decision

Added: researcher scaling rationale and 7-day pattern analysis.
```

This creates a persistent, auditable record of the agent's working state across restarts — the agent never starts from scratch.

## Configuration

Persona files are declared in the `.wh` and versioned in the infrastructure git repo:

```yaml
agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main, ops]
    persona: agents/donna/   # path to persona files in git
    max_replicas: 2
```

## Inspection

```sh
wh persona donna          # show current persona summary
wh persona donna --file MEMORY.md  # show specific file
```
