---
title: CLI Reference
description: Complete reference for the wh command-line interface
---

The `wh` CLI is the primary control plane for Wheelhouse — used by human operators and agents alike.

## Global flags

| Flag | Description |
|------|-------------|
| `--format json` | Output as JSON (all commands) |
| `--no-color` | Strip ANSI colors; Unicode symbols preserved |
| `--help` | Show help (works offline) |

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Error |
| `2` | Plan change detected (`wh deploy plan`) |

## Commands

| Command | Description |
|---------|-------------|
| `wh deploy lint <file>` | Validate `.wh` syntax |
| `wh deploy plan <file>` | Preview topology changes |
| `wh deploy apply <file>` | Apply a topology |
| `wh deploy destroy <file>` | Destroy a topology |
| `wh ps` | List running components (agents + surfaces) |
| `wh logs <agent>` | Stream agent logs |
| `wh status` | Topology health summary |
| `wh stream create <name>` | Create a stream |
| `wh stream list` | List streams |
| `wh stream delete <name>` | Delete a stream |
| `wh stream tail <name>` | Live stream of objects |
| `wh surface cli --stream <name>` | Interactive CLI surface (PUB/SUB to broker) |
| `wh secrets init` | Initialize credential wizard |
| `wh memory` | Show agent memory (MEMORY.md) |
| `wh compact` | Trigger stream compaction |
| `wh doctor` | Check topology and git health |
| `wh completion <shell>` | Generate shell completion |

## Output formats

### `wh ps`

```
  NAME              STATUS    STREAM    PROVIDER    UPTIME
  ──────────────────────────────────────────────────────────
  researcher        running   main      podman      2d 14h
  summarizer        running   main      podman      2d 14h
! watcher           stopped   alerts    podman      —

  3 agents  ·  2 running  ·  1 stopped
```

`!` prefix + column highlight mark a stopped or degraded agent. Column structure is frozen — builds muscle memory.

With `--format json`:

```json
{
  "components": [
    { "name": "researcher", "status": "running", "stream": "main", "provider": "podman", "uptime": "2d 14h" }
  ],
  "summary": { "total": 3, "running": 2, "stopped": 1 }
}
```

### `wh deploy plan`

```
  Changes to apply:

  + agent researcher          (new)     podman · claude-3-5-sonnet
  ~ agent summarizer          (update)  replicas: 1 → 2
  ! stream legacy-alerts      (destroy)

  1 to create · 1 to update · 1 to destroy

  Run 'wh deploy apply topology.wh' to apply these changes.
```

Prefix legend: `+` create · `~` update · `!` destroy.

With `--format json`, the response includes `has_changes: bool` as a top-level field for agent-readable consumption:

```json
{
  "has_changes": true,
  "changes": [
    { "op": "create", "kind": "agent", "name": "researcher" },
    { "op": "update", "kind": "agent", "name": "summarizer", "diff": { "replicas": [1, 2] } },
    { "op": "destroy", "kind": "stream", "name": "legacy-alerts" }
  ]
}
```

### `wh deploy lint` errors

Lint errors use compiler-style format — `file:line: field 'X' — reason`:

```
topology.wh:14: field 'max_replicas' is required — prevents unconstrained autonomous scaling
topology.wh:8: field 'retention' — expected duration string (e.g. "30d"), got integer
```

### `wh stream tail`

Each line: `[ISO8601] [TypeName] [publisher] content`

```
[2026-03-12T10:00:01Z] [TextMessage]     [donna]      Hello — I'm ready.
[2026-03-12T10:00:03Z] [SkillInvocation] [researcher] {"skill":"summarize","query":"..."}
[2026-03-12T10:00:04Z] [SkillResult]     [summarize]  {"result":"Summary: ..."}
```

Filter by type with `--filter type=<TypeName>`:

```sh
wh stream tail main --filter type=TextMessage
```

Content is truncated at 120 characters. Use `--verbose` to disable truncation.

## Machine-readable output

`--format json` is a first-class output contract, not an afterthought. Every command's JSON schema is stable — breaking changes require a major version bump.

Designed for agent consumption: `wh deploy plan --format json` gives agents a structured decision context. `wh ps --format json` gives monitoring pipelines machine-parseable status.

```sh
# Check if topology has pending changes (agent use)
wh deploy plan topology.wh --format json | jq '.has_changes'

# Get running agent names
wh ps --format json | jq '[.components[] | select(.status == "running") | .name]'
```

### `wh surface cli`

Connect to a stream as an interactive terminal surface. Probes broker liveness before connecting; reconnects automatically on transient failures.

```sh
wh surface cli --stream main
wh surface cli --stream main --format json
```

```
Connected to stream 'main'. Type a message and press Enter. Ctrl+C to quit.
> Hello
[donna] Hi! How can I help?
```

With `--format json`, each incoming message is printed as a JSON object:

```json
{ "publisher": "donna", "type": "TextMessage", "content": "Hi! How can I help?" }
```

Endpoint env vars (defaults match broker defaults):

| Env var | Description |
|---------|-------------|
| `WH_PUB_ENDPOINT` | Override broker PUB socket address |
| `WH_SUB_ENDPOINT` | Override broker SUB socket address |
| `WH_CONTROL_ENDPOINT` | Override broker control socket (used for liveness probe) |

## Shell completion

```sh
wh completion bash >> ~/.bashrc
wh completion zsh >> ~/.zshrc
wh completion fish > ~/.config/fish/completions/wh.fish
```
