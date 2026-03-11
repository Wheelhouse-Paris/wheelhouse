---
title: Streams
description: The typed object bus at the heart of Wheelhouse
---

A **stream** is a real-time, multi-subscriber, typed object bus. It is the connective tissue of every Wheelhouse topology.

## Core capabilities

Every stream, regardless of provider, supports:

- **Pub/sub** — any number of publishers and subscribers
- **Persistence** — objects are stored according to retention configuration
- **Compaction** — periodic summarization via cron jobs (daily, weekly, monthly)
- **Git backup** — compaction summaries are committed to git

## Object types

Streams carry typed Protobuf objects. Base types shipped with Wheelhouse:

| Type | Description |
|------|-------------|
| `TextMessage` | Plain text message |
| `FileMessage` | File or binary payload |
| `Reaction` | Reaction to a previous object |
| `SkillInvocation` | Request to execute a skill |
| `SkillResult` | Result or error from a skill execution |
| `CronEvent` | Scheduled trigger from cron provider |

Custom surfaces can register their own types (e.g. `biotech.MoleculeObject`).

## Providers

| Provider | Pub/Sub | Historical Query | Semantic Search |
|----------|---------|-----------------|-----------------|
| `local` | ✅ | — | — |
| `elasticsearch` | ✅ | ✅ | — |
| `weaviate` | ✅ | ✅ | ✅ |

## Configuration

```yaml
streams:
  - name: main
    provider: local
    retention:
      max_age: 30d
```
