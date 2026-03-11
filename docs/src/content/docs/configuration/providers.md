---
title: Providers
description: Agent and stream providers in Wheelhouse
---

Providers are the execution backends for agents and streams. The same `.wh` topology deploys identically across all providers.

## Agent providers

| Provider | Status | Description |
|----------|--------|-------------|
| `podman` | ✅ MVP | Local container runtime (Apache 2.0) |
| `aws-bedrock` | Phase 2 | AWS managed agents |
| `azure` | Phase 2 | Azure AI agents |

## Stream providers

| Provider | Status | Pub/Sub | Historical | Semantic |
|----------|--------|---------|-----------|---------|
| `local` | ✅ MVP | ✅ | — | — |
| `elasticsearch` | Phase 2 | ✅ | ✅ | — |
| `weaviate` | Phase 2 | ✅ | ✅ | ✅ |

## Configuration

```yaml
streams:
  - name: main
    provider: local

agents:
  - name: donna
    provider: podman   # default
```
