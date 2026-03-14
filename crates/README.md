# Crates

Wheelhouse is organized as a Cargo workspace with `wh-` prefixed crates:

- **wh-broker** — Message broker: ZMQ XPUB/XSUB routing, WAL persistence, type registry, cron scheduler
- **wh-cli** — CLI binary (`wh`): unified control plane for operators and agents
- **wh-proto** — Protobuf type definitions shared by all crates (single `include!` point)
