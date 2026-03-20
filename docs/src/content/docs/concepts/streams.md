---
title: Streams
description: The typed object bus at the heart of Wheelhouse
---

A **stream** is a real-time, multi-subscriber, typed object bus. It is the connective tissue of every Wheelhouse topology.

## Core capabilities

Every stream supports:

- **Pub/sub** — any number of publishers and subscribers
- **Persistence** — objects stored in a local append log; no objects lost on restart within the configured retention limit
- **Compaction** — periodic summarization via cron jobs (daily, weekly, monthly); summaries committed to git
- **Git backup** — compaction summaries form an auditable, versioned history

## Object types

Streams carry typed objects. Core types shipped with Wheelhouse:

| Type | Description |
|------|-------------|
| `TextMessage` | Plain text message |
| `FileMessage` | File or binary payload |
| `SkillInvocation` | Request to execute a skill |
| `SkillResult` | Result or error from a skill execution |
| `SkillProgress` | Progress signal from a long-running skill |
| `CronEvent` | Scheduled trigger from a cron job |
| `TopologyShutdown` | System event published before a clean topology stop |

Custom surfaces can register their own types (e.g. `biotech.MoleculeObject`). The `wheelhouse.*` namespace is reserved.

Stream messages from human users carry a `user_id` field referencing a registered User — enabling attribution, auditing, and GDPR-compliant data management.

## Providers

Currently implemented: **local** (WAL-backed, in-process).

Multi-provider support (elasticsearch, weaviate) with historical query and semantic search is planned for Phase 2.

## Configuration

```yaml
streams:
  - name: main
    retention: "30d"   # optional duration string; omit to keep forever
```

A stream without a compaction cron generates a lint warning at `wh topology lint` time — objects will accumulate without bound.
