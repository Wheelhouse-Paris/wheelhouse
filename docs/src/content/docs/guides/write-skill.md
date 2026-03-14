---
title: Write a skill
description: Create a versioned skill that agents can invoke
---

A skill is a versioned recipe stored in git. Agents invoke it by publishing a `SkillInvocation` object to a stream — no LLM-specific tool-calling format required.

## Skill structure

```
skills/
  my-skill/
    skill.md
    steps.md
```

### skill.md

```markdown
---
name: my-skill
version: 1.0.0
inputs:
  query: string
outputs:
  result: string
---

# My Skill

Description of what this skill does.
```

### steps.md

```markdown
# Steps

1. Read `query` from the SkillInvocation payload
2. Process the query
3. Publish a `SkillResult` with `result`

## Error handling

Always publish a `SkillResult` — even on failure. Silent timeouts are forbidden.
```

## How agents invoke skills

Agents publish a `SkillInvocation` protobuf message to the stream. The broker routes it to the registered skill handler. The skill publishes a `SkillResult` (or `SkillProgress` for long-running tasks) back to the stream.

These types (`SkillInvocation`, `SkillResult`, `SkillProgress`) are defined in `proto/wheelhouse/v1/` and are currently available on the **Rust side only**. Python SDK bindings are planned for Phase 2.

## Skill storage

Skills are stored in git repositories and loaded lazily by the broker at invocation time. A skill reference pins a specific commit hash — branch references are rejected.

## Declaring skills in `.wh`

Declare skills in the agent's `.wh` entry. Skill references must be pinned to a commit hash — branch names are rejected to prevent supply chain drift.

```yaml
agents:
  - name: donna
    skills_repo: github.com/you/your-skills
    skills:
      - name: my-skill
        ref: a1b2c3d4e5f6   # pinned commit hash, not a branch
```

The broker loads each skill lazily on first invocation. An agent can only invoke skills listed in its `.wh` entry — undeclared invocations are rejected.
