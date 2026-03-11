---
title: CLI Reference
description: Complete reference for the wh command-line interface
---

The `wh` CLI is the primary control plane for Wheelhouse — used by human operators and agents alike.

## Global flags

| Flag | Description |
|------|-------------|
| `--format json` | Output as JSON (all commands) |
| `--quiet` | Suppress non-essential output |
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
| `wh deploy apply <file>` | Apply a topology |
| `wh deploy plan <file>` | Preview topology changes |
| `wh deploy destroy <file>` | Destroy a topology |
| `wh deploy lint <file>` | Validate `.wh` syntax |
| `wh ps` | List running components |
| `wh ls` | List all deployed topologies |
| `wh logs <agent>` | Stream agent logs |
| `wh status` | Broker health and metrics |
| `wh restart` | Restart the broker |
| `wh stream tail <name>` | Live stream of objects |
| `wh secrets init` | Initialize credential wizard |
| `wh completion <shell>` | Generate shell completion |

## Shell completion

```sh
wh completion bash >> ~/.bashrc
wh completion zsh >> ~/.zshrc
wh completion fish > ~/.config/fish/completions/wh.fish
```
