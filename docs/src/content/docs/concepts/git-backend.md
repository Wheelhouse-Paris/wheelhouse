---
title: Git Backend
description: Infrastructure configuration versioned in git
---

Wheelhouse uses git as the storage backend for configuration and compaction summaries.

## What is versioned

| Domain | Files |
|--------|-------|
| Personas | `SOUL.md`, `IDENTITY.md`, `MEMORY.md` |
| TTY meta | `tty/{name}/meta.json`, `CLAUDE.md` |
| Skills | `skills/{name}/skill.md`, `*.steps` |
| Cron | `cron/jobs.yaml` |
| Users | `users/*.json` |
| Telegram config | `telegram/*.json` (no `bot_token`) |

## What is NOT versioned

- Channel messages and cursors (runtime state)
- Agent/surface sessions (ephemeral)
- Workflow run logs
- Secrets of any kind

## Agent-attributed commits

Every infrastructure change is committed with the agent's name:

```
git log --oneline
a3f9c2e donna: scale researcher to 2 replicas — 4 timeouts detected
7b2d1a0 donna: cap telegram summaries at 380 chars
```

## Infrastructure portability

Migrate a complete infrastructure to a new machine:

```sh
git clone https://github.com/you/your-infra
wh deploy apply topology.wh
```
