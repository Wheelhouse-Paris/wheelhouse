---
title: Skills
description: Versioned recipes stored in git, invoked via stream objects
---

A **skill** is a versioned recipe — a set of markdown files and steps stored in git — that an agent can invoke by publishing a `SkillInvocation` object into a stream.

Skills are deliberately decoupled from any LLM's native tool-calling format. The stream object *is* the invocation mechanism.

## Structure

```
skills/
  web-search/
    skill.md        # description, inputs, outputs
    steps.md        # execution steps
    requirements.txt  # optional dependencies
```

## Invocation

```python
await stream.publish(SkillInvocation(
    skill="web-search",
    input={"query": "latest ZMQ Rust bindings"},
    reply_to="main"
))
```

The agent picks up the `SkillInvocation`, executes the skill, and publishes a `SkillResult` — or an error object if it fails. **Silent timeouts are forbidden.**

## Declaring skills in `.wh`

```yaml
agents:
  - name: donna
    skills:
      - repo: github.com/wheelhouse-paris/skills
        name: web-search
        ref: v1.2.0
```

Skills are lazy-loaded on first invocation.
