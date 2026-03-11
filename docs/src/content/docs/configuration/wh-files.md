---
title: .wh Files
description: Declarative topology configuration for Wheelhouse
---

A `.wh` file is a declarative YAML topology definition — the Dockerfile of agentic infrastructure.

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
        ref: <tag-or-sha>

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

## Validation

Validate syntax without applying:

```sh
wh deploy lint topology.wh
```

Preview changes before applying:

```sh
wh deploy plan topology.wh
```

## Guardrails

`max_replicas` is mandatory per agent. The deployment is blocked if exceeded.

Additional guardrails planned for Phase 2: rate limiting on autonomous apply, anomaly detection on destructive plans.
