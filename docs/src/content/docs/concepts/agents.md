---
title: Agents
description: Autonomous operators that observe, decide, and act
---

An **agent** is an autonomous process that subscribes to streams, makes decisions, and publishes results — including modifications to its own infrastructure.

## Startup sequence

An agent loads its [persona](/concepts/personas) before connecting to any stream:

```
load persona (SOUL.md → IDENTITY.md → MEMORY.md) → connect to streams → begin loop
```

## The autonomous loop

```
observe (stream) → decide (LLM) → act (publish / wh deploy apply)
```

An agent can revise its own `.wh` file at any time based on any signal it receives — a stream message, a compaction summary, a user command, or its own judgment.

## Stream messages are untrusted input

An agent must treat stream messages as untrusted user input. Infrastructure-modifying operations (e.g. `wh deploy apply`) must not be triggered solely by stream message content — they require a secondary validation step: human confirmation or a policy check defined in `wh-policy.yaml`.

## Identity & audit trail

Every autonomous apply is attributed in git. The commit message format is:

```
[agent-name] apply: <summary>

Plan: <plan-output-or-hash>
```

Example:

```
[donna] apply: scale researcher to 2 replicas

Plan: ~ agent researcher / replicas: 1 → 2
Reason: 4 daily timeouts detected over 6 days.
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

`max_replicas` is mandatory — it prevents unconstrained autonomous scaling.

Operator safety limits (validation thresholds, apply rate limits) are configured in `wh-policy.yaml`, not in the agent's `.wh` file. This separation ensures agents cannot modify their own guardrails.

Additional guardrails planned for Phase 2: rate limiting on autonomous apply, anomaly detection on destructive plans.
