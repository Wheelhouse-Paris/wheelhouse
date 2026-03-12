---
title: .wh Files
description: Declarative topology configuration for Wheelhouse
---

A `.wh` file is a declarative YAML topology definition — the Dockerfile of agentic infrastructure.

## Structure

```yaml
apiVersion: wheelhouse.dev/v1
name: <topology-name>

streams:
  - name: <stream-name>
    retention: <duration>    # e.g. "7d", "30d" (optional)

agents:
  - name: <agent-name>
    image: <container-image>
    replicas: <n>            # default: 1
    streams: [<stream-names>]
    persona: <path-to-persona-dir>  # e.g. agents/donna/

guardrails:
  max_replicas: <n>          # caps replicas across all agents in this topology
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
