---
title: Write a skill
description: Create a versioned skill that agents can invoke
---

A skill is a versioned recipe stored in git. Agents invoke it by publishing a `SkillInvocation` object — no LLM-specific tool-calling format required.

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

## Declare in your .wh file

```yaml
agents:
  - name: donna
    skills:
      - name: my-skill
        repo: github.com/you/your-skills
        ref: v1.0.0
```

## Invoke from a surface

```python
await stream.publish(SkillInvocation(
    skill="my-skill",
    input={"query": "hello"},
    reply_to="main"
))
```

## Receive the result

```python
async for obj in stream.subscribe():
    if isinstance(obj, SkillResult):
        print(obj.output)
    elif isinstance(obj, SkillError):
        print(f"Skill failed: {obj.error}")
```
