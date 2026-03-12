---
title: CLI Reference
description: Complete reference for the wh command-line interface
---

The `wh` CLI is the primary control plane for Wheelhouse — used by human operators and agents alike.

## Global flags

| Flag | Description |
|------|-------------|
| `--format json` | Output as JSON (all commands) |
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
| `wh ps` | List running components |
| `wh logs <agent>` | Stream agent logs |
| `wh status` | Topology health summary |
| `wh stream create <name>` | Create a stream |
| `wh stream list` | List streams |
| `wh stream delete <name>` | Delete a stream |
| `wh stream tail <name>` | Live stream of objects |
| `wh secrets init` | Initialize credential wizard |
| `wh memory` | Show agent memory (MEMORY.md) |
| `wh compact` | Trigger stream compaction |
| `wh doctor` | Check topology and git health |
| `wh completion <shell>` | Generate shell completion |

## Shell completion

```sh
wh completion bash >> ~/.bashrc
wh completion zsh >> ~/.zshrc
wh completion fish > ~/.config/fish/completions/wh.fish
```
