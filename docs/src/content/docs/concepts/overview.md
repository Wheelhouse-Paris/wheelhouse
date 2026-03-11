---
title: Architecture Overview
description: How Wheelhouse components fit together
---

Wheelhouse is built around a central typed object bus — the **stream** — around which all components gravitate.

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│    Agent    │────▶│   Stream    │◀────│   Surface   │
│ (publisher/ │     │ (ZMQ broker)│     │  (Telegram, │
│ subscriber) │◀────│             │────▶│  CLI, custom│
└─────────────┘     └──────┬──────┘     └─────────────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         ┌────────┐  ┌──────────┐  ┌────────┐
         │  Cron  │  │  Skills  │  │  Git   │
         │ events │  │  (lazy)  │  │backend │
         └────────┘  └──────────┘  └────────┘
```

The broker implements ZMQ XPUB/XSUB — any number of publishers and subscribers can participate in a stream simultaneously.

See individual concept pages for details on each component.
