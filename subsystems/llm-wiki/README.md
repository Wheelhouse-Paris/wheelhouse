# LLM-Wiki Subsystem

The LLM-Wiki subsystem adds autonomous knowledge persistence to your Wheelhouse topology. A dedicated librarian agent observes conversations from member agents and decides what to save to a shared, git-versioned Library.

## How it works

1. Member agents have conversations with users as normal
2. At the end of each turn, a `LibraryWriteEvent` is published to a dedicated stream
3. The librarian agent receives the event and evaluates the conversation segment
4. If durable knowledge is detected, the librarian writes a page to the Library and commits to git
5. Member agents can search and cite the Library (read-only access)

## Quick start

Add the subsystem to your `.wh` topology file:

```yaml
api_version: wheelhouse.dev/v1
name: my-topology

agents:
  - name: assistant
    image: ghcr.io/wheelhouse-paris/agent-claude:latest
    streams: [main]

streams:
  - name: main

surfaces:
  - name: cli
    kind: cli
    stream: main

subsystems:
  - path: subsystems/llm-wiki/
    members: [assistant]
    library_name: knowledge
```

Then deploy:

```bash
wh deploy plan my-topology.wh    # Preview what will be created
wh deploy apply my-topology.wh   # Provision everything
```

The composition loader expands the subsystem declaration into:

- A `librarian-knowledge` agent container with read-write access to the Library volume
- An `eot-assistant-knowledge` end-of-turn stream
- A `wh-my-topology-llm-wiki-knowledge` named volume
- A read-only Library volume mount on the `assistant` agent
- `WH_LIBRARY_WRITE_STREAM` environment variable on the `assistant` agent

## Parameters

| Parameter | Required | Default | Description |
|---|---|---|---|
| `path` | yes | -- | Must be `subsystems/llm-wiki/` |
| `members` | yes | -- | List of agent names that participate in the Library |
| `library_name` | no | topology name | Name for the Library (used in volume and stream names) |
| `locales` | no | -- | Locale codes for decision prompts (e.g., `[en, fr]`) |
| `decision_prompt` | no | -- | Path to a custom decision prompt file |

## Multi-agent example

Multiple agents can share the same Library:

```yaml
subsystems:
  - path: subsystems/llm-wiki/
    members: [agent-a, agent-b, agent-c]
    library_name: research
    locales: [en, fr]
```

This creates:

- 1 librarian agent (`librarian-research`) with read-write access
- 3 end-of-turn streams (one per member)
- 3 read-only Library mounts (one per member agent)
- 1 shared named volume (`wh-<topo>-llm-wiki-research`)

All agents contribute knowledge to the same Library. The librarian attributes each page to its source agent.

## Folder contents

| File | Purpose |
|---|---|
| `composition.wh` | Template documentation showing the expansion pattern |
| `librarian-persona.md` | Operational context for the librarian agent |
| `prompts/decision_en.md` | English decision prompt |
| `prompts/decision_fr.md` | French decision prompt |
| `schema-template-reader.md` | Read-only schema injected into member agents |
| `README.md` | This file |

## Lifecycle management

```bash
# Preview resources
wh deploy plan my-topology.wh

# Create everything
wh deploy apply my-topology.wh

# Tear down (removes containers and streams; volumes preserved by default)
wh deploy destroy my-topology.wh

# Tear down including Library volume
wh deploy destroy my-topology.wh --remove-volumes
```

## Monitoring

```bash
# Stream librarian decision logs
wh logs librarian-knowledge

# View Library commit history
podman exec librarian-knowledge git -C /workspace/.library log --oneline

# Check Library page count
podman exec librarian-knowledge find /workspace/.library/pages -name "*.md" | wc -l
```

## Troubleshooting

**Librarian not receiving events**: Check that `WH_STREAMS` is correctly set on the librarian container. Verify member agents have `WH_LIBRARY_WRITE_STREAM` in their environment.

**Write attempts failing on member agents**: This is expected. Member agents have read-only access (EROFS). Only the librarian can write.

**No pages being written**: Check librarian logs for decision reasons. The librarian may be correctly skipping transient content. Review `decision_en.md`/`decision_fr.md` prompts.

**Missing ANTHROPIC_API_KEY**: The librarian requires an API key for LLM decision calls. Ensure it is configured via `wh secrets init` or environment variable.
