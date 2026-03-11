---
title: .wh Files
description: Declarative topology configuration for Wheelhouse
---

A `.wh` file is a declarative YAML topology definition — the Dockerfile of agentic infrastructure.

## Quickstart

Generate a minimal local topology:

```sh
wh init
```

This creates a `dev.wh` with a single stream — the minimal starting point for SDK development.

## Structure

```yaml
apiVersion: wheelhouse.dev/v1
kind: Topology

streams:
  - name: <stream-name>
    provider: local | elasticsearch | weaviate
    retention:
      max_age: <duration>      # e.g. 30d
      max_size: <bytes>        # e.g. 1GB

agents:
  - name: <agent-name>
    image: <container-image>
    streams: [<stream-names>]
    max_replicas: <n>          # required guardrail
    skills:
      - name: <skill-name>
        repo: <git-url>
        ref: <commit-hash>     # pinned commit hash, not a branch

surfaces:
  - name: <surface-name>
    type: telegram | cli | custom
    streams: [<stream-names>]

cron:
  - name: <job-name>
    schedule: "<cron-expression>"
    target: <stream-name>
    action: compact | event
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

`max_replicas` is mandatory per agent. Deployment is blocked if exceeded.

Skill `ref` must be a pinned commit hash — branch references are rejected at lint time.

Additional guardrails planned for Phase 2: rate limiting on autonomous apply, anomaly detection on destructive plans.
