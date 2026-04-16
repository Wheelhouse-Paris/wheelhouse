# Librarian Persona

The librarian is an autonomous agent that processes end-of-turn conversation segments and decides whether to persist durable knowledge to the Library.

## Role

The librarian operates as a **background process** in the topology. It does not interact directly with end users. Instead, it subscribes to end-of-turn event streams produced by member agents and evaluates each segment for durable knowledge.

## Responsibilities

1. **Receive** `LibraryWriteEvent` messages from member agent streams
2. **Evaluate** each conversation segment using a locale-aware decision prompt
3. **Write** durable facts to the Library as git-committed markdown pages
4. **Skip** transient content, secrets, and non-durable PII
5. **Update** existing pages when new information supersedes prior knowledge
6. **Merge** duplicate content into existing pages
7. **Log** every decision as a structured JSON entry with a V1 reason code

## Decision Framework

The librarian uses decision prompts located in `subsystems/llm-wiki/prompts/`:

- `decision_en.md` -- English decision prompt
- `decision_fr.md` -- French decision prompt

Each prompt evaluates the segment against these criteria:

| Reason Code | Committed | Meaning |
|---|---|---|
| `durable_fact_written` | true | New durable fact persisted |
| `update_existing_page` | true | Existing page updated with new information |
| `dedup_merged` | true | Duplicate merged into existing page |
| `no_durable_fact_detected` | false | No durable fact found in segment |
| `pii_not_durable` | false | PII flagged as non-durable |
| `contains_secret_pattern` | false | Secret or credential pattern detected |
| `transient_context` | false | Conversational filler (greetings, acks) |

Runtime-only codes (not produced by the decision prompt):

| Reason Code | Committed | Meaning |
|---|---|---|
| `dedup_skipped` | false | Event already processed (dedup cache hit) |
| `decision_timeout` | false | LLM call exceeded 10s timeout |
| `decision_error` | false | LLM response could not be parsed |

## Monitoring

The librarian emits structured JSON decision logs to stdout. Each log entry contains:

- `schema_version`: always `1`
- `event_id`: UUID of the processed event
- `library_id`: Library identifier
- `source_agent_id`: originating agent name
- `reason`: V1 reason code
- `committed`: boolean
- `commit_hash`: git SHA if committed, null otherwise
- `locale`: detected locale of the segment
- `tokens_consumed`: LLM tokens used
- `duration_ms`: total processing time
- `step_spans`: array of timing spans (dedup, prompt, llm, git)

Use `wh logs librarian-<library_name>` to stream these logs in real time.
Use `git log` on the Library volume to see commit history with structured metadata.

## Configuration

The librarian container receives its configuration via environment variables:

| Variable | Description |
|---|---|
| `WH_AGENT_NAME` | Container name (e.g., `librarian-research`) |
| `WH_STREAMS` | Comma-separated list of event streams to subscribe to |
| `WH_LIBRARY_PATH` | Mount path to the Library volume (`/workspace/.library`) |
| `WH_LIBRARY_ID` | Library identifier for logging and attribution |
| `WH_LIBRARIAN_LOCALES` | Comma-separated locale codes (e.g., `en,fr`) |
| `WH_DECISION_PROMPT` | Optional custom decision prompt path |
| `ANTHROPIC_API_KEY` | API key for the LLM decision calls |
| `WH_URL` | Broker connection URL |
