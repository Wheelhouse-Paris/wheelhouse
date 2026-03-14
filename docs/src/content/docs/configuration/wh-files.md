---
title: .wh Files
description: Declarative topology configuration for Wheelhouse
---

A `.wh` file is a declarative YAML topology definition — the Dockerfile of agentic infrastructure.

## Structure

```yaml
# topology.wh — complete example
api_version: wheelhouse.dev/v1
name: my-topology

streams:
  - name: main
    retention: "30d"    # optional; omit to keep forever

agents:
  - name: donna
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    replicas: 1                 # default: 1
    streams: [main]             # streams this agent subscribes to
    persona: agents/donna/      # optional: path to SOUL.md / IDENTITY.md / MEMORY.md
    skills_repo: github.com/org/skills-repo   # optional: git repo containing skills
    skills:                     # optional: skills this agent is allowed to invoke
      - name: web-search
        ref: a3f9c2d            # pinned commit hash — branch refs are rejected

surfaces:
  - name: telegram
    kind: telegram              # telegram | cli | custom
    image: ghcr.io/wheelhouse-paris/wh-telegram:latest
    stream: main                # stream this surface exchanges messages with
    env:                        # optional: additional env vars (non-secret config only)
      WH_SURFACE_NAME: telegram

guardrails:
  max_replicas: 2    # topology-wide cap — deployment blocked if any agent exceeds this
```

## Agents

| Field | Required | Description |
|-------|----------|-------------|
| `name` | ✓ | Container name (lowercase, alphanumeric, hyphens) |
| `image` | ✓ | OCI image reference |
| `replicas` | | Number of instances (default: 1) |
| `streams` | | List of stream names to subscribe to |
| `persona` | | Path to persona directory (`SOUL.md`, `IDENTITY.md`, `MEMORY.md`) |
| `skills_repo` | | Git repository URL containing skill definitions |
| `skills` | | List of pinned skill references (requires `skills_repo`) |

Each skill entry has `name` and `ref` (pinned commit hash). An agent can only invoke skills declared in its `.wh` entry — undeclared invocations are rejected by the broker.

## Surfaces

Surfaces connect human users to streams. The provisioning layer starts and stops surface containers automatically alongside agents.

| Field | Required | Description |
|-------|----------|-------------|
| `name` | ✓ | Surface identifier (lowercase, alphanumeric, hyphens) |
| `kind` | ✓ | `telegram`, `cli`, or `custom` |
| `image` | ✓ | OCI image for the surface container |
| `stream` | ✓ | Stream this surface exchanges messages with |
| `env` | | Additional environment variables (non-secret config) |

The provisioning layer automatically injects these env vars into every surface container — no manual config required:

| Env var | Value |
|---------|-------|
| `WH_URL` | Broker ZMQ endpoint |
| `WH_SURFACE_NAME` | Value of `name` from the topology |
| `WH_STREAM` | Value of `stream` from the topology |

Secrets (Telegram bot token, API keys) are stored via `wh secrets init` and injected automatically at apply time — they do not belong in the `env:` block.

## Streams

| Field | Required | Description |
|-------|----------|-------------|
| `name` | ✓ | Stream name (`[a-z][a-z0-9-]*`) |
| `retention` | | Duration string (e.g. `"30d"`, `"7d"`) — omit to keep forever |

## Operator safety policy

Guardrails that agents must not be able to modify live in a separate `wh-policy.yaml` file owned by the operator:

```yaml
# wh-policy.yaml
agents:
  donna:
    human_validation_threshold: 0.8
    max_apply_per_hour: 10
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
