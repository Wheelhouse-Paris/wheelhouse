---
title: Agents
description: Autonomous operators that observe, decide, and act
---

An **agent** is an autonomous process that subscribes to streams, makes decisions, and publishes results — including modifications to its own infrastructure.

## The autonomous loop

```
observe (stream) → decide (LLM) → act (publish / wh deploy apply)
```

An agent can revise its own `.wh` file at any time based on any signal it receives — a stream message, a compaction summary, a user command, or its own judgment.

## Identity

In MVP, each agent is identified by name in git commit messages:

```
feat(infra): scale researcher to 2 replicas

Reason: 4 daily timeouts detected over 6 days.
Agent: donna
```

GPG-signed commits with cryptographic PKI are planned for Phase 2.

## Configuration

```yaml
agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main, ops]
    max_replicas: 2
    skills: [summarize, web-search]
```

## Guardrails

`max_replicas` is mandatory — it prevents unconstrained autonomous scaling. Additional guardrails (rate limit on autonomous apply, anomaly detection) are planned for Phase 2.
