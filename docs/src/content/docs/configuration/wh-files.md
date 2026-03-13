---
title: .wh Files
description: Declarative topology configuration for Wheelhouse
---

A `.wh` file is a declarative YAML topology definition — the Dockerfile of agentic infrastructure.

## Structure

```yaml
# topology.wh — minimal working example
api_version: wheelhouse.dev/v1
name: my-topology

# Streams are the typed message bus connecting all components.
# Each stream is an append log; objects are retained for the specified duration.
streams:
  - name: main
    retention: "30d"    # optional duration string; omit to keep forever
                        # a stream without a compaction cron generates a lint warning

# Agents subscribe to streams, make decisions, and publish results.
# They can also modify this file autonomously via wh deploy apply.
agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    replicas: 1                 # default: 1
    streams: [main]             # list of streams this agent subscribes to
    persona: agents/donna/      # optional: path to SOUL.md / IDENTITY.md / MEMORY.md

# Guardrails live here, not on individual agents.
# This separation ensures agents cannot modify their own constraints.
guardrails:
  max_replicas: 2    # topology-wide cap — deployment blocked if any agent exceeds this
```

## Operator safety policy

Guardrails that agents must not be able to modify — validation thresholds, apply rate limits — live in a separate `wh-policy.yaml` file owned by the operator:

```yaml
# wh-policy.yaml
agents:
  donna:
    human_validation_threshold: 0.8
    max_apply_per_hour: 10     # Phase 2
```

`wh deploy apply` validates against `wh-policy.yaml` before executing. Policy changes require explicit human confirmation regardless of threshold setting.

## Validation

Validate syntax without applying:

```sh
wh deploy lint topology.wh
```

Preview changes before applying:

```sh
wh deploy plan topology.wh
```

Check topology and git health:

```sh
wh doctor
```

## Guardrails

`max_replicas` in the `guardrails` block caps the maximum replicas allowed for any single agent in the topology. Deployment is blocked if exceeded.

Additional guardrails planned for Phase 2: rate limiting on autonomous apply, anomaly detection on destructive plans.

## Phase 2 — coming soon

The following fields are planned but not yet parsed:

- **`streams[].provider`** — storage backend (local / elasticsearch / weaviate)
- **`agents[].skills`** — pinned skill references from git
- **`surfaces`** — surface declarations (telegram, cli, custom)
- **`cron`** — scheduled job declarations
