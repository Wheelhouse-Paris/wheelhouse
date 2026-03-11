---
title: Guardrails
description: Safety constraints for autonomous agent operation
---

Guardrails prevent agents from taking actions that exceed their defined boundaries.

## max_replicas

Mandatory per agent. Blocks deployment if the replica count would exceed this value.

```yaml
agents:
  - name: donna
    max_replicas: 2   # required — deployment blocked if exceeded
```

## Phase 2 guardrails

- **Rate limit on autonomous apply** — max N apply operations per hour per agent
- **Anomaly detection** — detect aberrant plans (e.g. destroy all agents) before autonomous apply
- **Budget max** — block deployments that exceed a cost threshold
