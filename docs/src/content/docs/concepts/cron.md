---
title: Cron
description: First-class scheduled automation
---

Cron is a first-class infrastructure primitive in Wheelhouse. A cron job publishes a typed `CronEvent` object into a target stream on a schedule.

## Configuration

```yaml
cron:
  - name: daily-compaction
    schedule: "0 3 * * *"
    target: main
    action: compact

  - name: morning-briefing
    schedule: "0 8 * * 1-5"
    target: main
    action: event
    payload:
      type: briefing
```

## CronEvent

Every trigger publishes a `CronEvent` into the target stream:

```protobuf
message CronEvent {
  string job_name = 1;
  string action = 2;
  google.protobuf.Timestamp triggered_at = 3;
  map<string, string> payload = 4;
}
```

Agents subscribe to streams and react to `CronEvent` objects like any other stream object.

## Failure alerting

Any cron failure triggers an immediate Surface notification. Silent failures are not allowed.
