---
title: Architecture Overview
description: How Wheelhouse components fit together
---

Wheelhouse is built around a central typed object bus — the **stream** — around which all components gravitate.

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│    Agent    │────▶│   Stream    │◀────│   Surface   │
│ (publisher/ │     │  (pub/sub   │     │  (Telegram, │
│ subscriber) │◀────│    bus)     │────▶│  CLI, custom│
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         ┌────────┐  ┌──────────┐  ┌────────┐
         │  Cron  │  │  Skills  │  │  Git   │
         │ events │  │  (lazy)  │  │backend │
         └────────┘  └──────────┘  └────────┘
```

The stream implements ZMQ XPUB/XSUB — any number of publishers and subscribers can participate simultaneously.

See individual concept pages for details on each component.
