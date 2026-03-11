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

For long-running skills, the agent publishes `SkillProgress` objects at regular intervals so surfaces can show progress to the user.

## Declaring skills in `.wh`

Skill references must be pinned to a specific git commit hash — not a branch name. This ensures reproducibility and prevents supply chain drift.

```yaml
agents:
  - name: donna
    skills:
      - repo: github.com/wheelhouse-paris/skills
        name: web-search
        ref: a3f9c2d   # pinned commit hash, not a branch
```

Skills are lazy-loaded on first invocation. An agent can only invoke skills declared in its `.wh` — undeclared skill invocations are rejected.
