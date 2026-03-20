# wheelhouse

> The operating infrastructure for autonomous agent factories.

Specify, deploy, monitor, and let your agents operate their own infrastructure.

```sh
brew install wheelhouse-paris/tap/wh
wh secrets init
wh topology apply my-agent.wh
```

**Documentation:** [docs.wheelhouse.paris](https://docs.wheelhouse.paris)
**Website:** [wheelhouse.paris](https://wheelhouse.paris)

---

## Architecture

```
Stream (ZMQ XPUB/XSUB + Protobuf)
  ├── Agents    — observe → decide → act
  ├── Surfaces  — Telegram, CLI, custom (SDK)
  ├── Skills    — versioned recipes in git
  └── Cron      — CronEvent publisher
```

The `.wh` file is the Dockerfile of agentic infrastructure. Agents read it, modify it, and apply it autonomously.

## Status

🚧 Active development — v0.1 in progress

## License

Apache 2.0 — © 2026 The Wheelhouse Paris
