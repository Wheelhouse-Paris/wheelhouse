# LLM-Wiki Decision Trace

This document walks through a realistic session of the LLM-Wiki librarian, showing how it evaluates conversation segments and produces structured decision logs. Each of the 10 V1 reason codes is demonstrated at least once.

## Decision Log Format

Every librarian decision emits a JSON log entry to stdout (viewable via `wh logs librarian-<library_name>`). The fields are:

| Field | Type | Description |
|---|---|---|
| `schema_version` | int | Always `1` for V1 logs |
| `event_id` | string | UUID of the LibraryWriteEvent being processed |
| `library_id` | string | Library identifier (from `WH_LIBRARY_ID`) |
| `source_agent_id` | string | Agent that produced the conversation turn |
| `conversation_id` | string | Conversation session identifier |
| `reason` | string | V1 reason code (see below) |
| `committed` | bool | Whether a Library page was written |
| `commit_hash` | string/null | Git SHA if committed, null otherwise |
| `locale` | string | Detected locale of the segment |
| `tokens_consumed` | int | LLM tokens used for the decision |
| `snippet` | string | Truncated triggering message (max 200 chars) |
| `timestamp` | string | ISO 8601 timestamp of the decision |
| `duration_ms` | int | Total processing time in milliseconds |
| `step_spans` | array | Timing breakdown: dedup check, prompt load, LLM call, git commit |

## V1 Reason Codes

The 10 codes split into two categories:

### Prompt-Driven Codes (7)

These are produced by the LLM decision prompt evaluating a conversation segment.

| Code | Committed | Meaning |
|---|---|---|
| `durable_fact_written` | true | New durable fact persisted as a Library page |
| `update_existing_page` | true | Existing page updated with new or corrected information |
| `dedup_merged` | true | Duplicate content merged into an existing page |
| `no_durable_fact_detected` | false | Segment contains no fact worth persisting |
| `pii_not_durable` | false | PII detected and flagged as non-durable |
| `contains_secret_pattern` | false | API key, password, or credential pattern detected |
| `transient_context` | false | Greetings, acknowledgments, or conversational filler |

### Runtime-Only Codes (3)

These are produced by the librarian runtime before or instead of calling the LLM.

| Code | Committed | Meaning |
|---|---|---|
| `below_locale_confidence` | false | Locale detection confidence below threshold (0.6) |
| `decision_timeout` | false | LLM call exceeded 10-second timeout |
| `decision_error` | false | LLM response could not be parsed as valid JSON |

---

## Trace Examples

The following examples show a sequence of events processed by the librarian in the `llm-wiki-demo` topology. Each example includes the conversation segment, the decision, and the full JSON log entry.

---

### 1. `durable_fact_written` -- New Fact Persisted

**Conversation segment** (from `assistant-en`):

> **User**: I just decided to postpone the Series A fundraise to Q3 2027. We want to hit 500k ARR first.
>
> **Assistant**: Got it. I've noted that you're postponing the Series A to Q3 2027, targeting 500k ARR as the threshold before raising.

**Decision**: The librarian detects a durable business fact (fundraise timeline and ARR target) and writes a new page.

```json
{
  "schema_version": 1,
  "event_id": "e1a2b3c4-d5e6-7890-abcd-ef1234567890",
  "library_id": "knowledge",
  "source_agent_id": "assistant-en",
  "conversation_id": "conv-001",
  "reason": "durable_fact_written",
  "committed": true,
  "commit_hash": "a1b2c3d",
  "locale": "en",
  "tokens_consumed": 342,
  "snippet": "I just decided to postpone the Series A fundraise to Q3 2027. We want to hit 500k ARR first.",
  "timestamp": "2026-04-16T10:00:01Z",
  "duration_ms": 1850,
  "step_spans": [
    {"step": "dedup_check", "ms": 2},
    {"step": "prompt_load", "ms": 15},
    {"step": "llm_call", "ms": 1780},
    {"step": "git_commit", "ms": 53}
  ]
}
```

**Library page created** (`pages/fundraise-timeline.md`):

```markdown
---
source_agent_id: assistant-en
conversation_id: conv-001
timestamp: 2026-04-16T10:00:01Z
snippet: I just decided to postpone the Series A fundraise to Q3 2027.
---

# Fundraise Timeline

Series A fundraise postponed to Q3 2027. Target: 500k ARR before raising.
```

---

### 2. `transient_context` -- Conversational Filler Skipped

**Conversation segment** (from `assistant-en`):

> **User**: Hi there! How are you doing today?
>
> **Assistant**: Hello! I'm doing well, thanks for asking. How can I help you today?

**Decision**: Pure greeting exchange with no durable information. Skipped.

```json
{
  "schema_version": 1,
  "event_id": "f2b3c4d5-e6f7-8901-bcde-f12345678901",
  "library_id": "knowledge",
  "source_agent_id": "assistant-en",
  "conversation_id": "conv-002",
  "reason": "transient_context",
  "committed": false,
  "commit_hash": null,
  "locale": "en",
  "tokens_consumed": 218,
  "snippet": "Hi there! How are you doing today?",
  "timestamp": "2026-04-16T10:01:15Z",
  "duration_ms": 920,
  "step_spans": [
    {"step": "dedup_check", "ms": 1},
    {"step": "prompt_load", "ms": 12},
    {"step": "llm_call", "ms": 907}
  ]
}
```

---

### 3. `contains_secret_pattern` -- Credential Detected

**Conversation segment** (from `assistant-en`):

> **User**: Here's my API key for the deployment: sk-proj-abc123def456ghi789jkl012mno345pqr678stu901vwx234yz
>
> **Assistant**: I see you've shared an API key. Please be careful with sensitive credentials.

**Decision**: The segment contains an API key pattern. The librarian refuses to persist it.

```json
{
  "schema_version": 1,
  "event_id": "a3c4d5e6-f7a8-9012-cdef-a12345678902",
  "library_id": "knowledge",
  "source_agent_id": "assistant-en",
  "conversation_id": "conv-003",
  "reason": "contains_secret_pattern",
  "committed": false,
  "commit_hash": null,
  "locale": "en",
  "tokens_consumed": 256,
  "snippet": "Here's my API key for the deployment: sk-proj-abc123def456ghi789jkl012mno345pqr678stu901vwx234yz",
  "timestamp": "2026-04-16T10:02:30Z",
  "duration_ms": 1100,
  "step_spans": [
    {"step": "dedup_check", "ms": 1},
    {"step": "prompt_load", "ms": 14},
    {"step": "llm_call", "ms": 1085}
  ]
}
```

---

### 4. `no_durable_fact_detected` -- General Knowledge, Nothing to Persist

**Conversation segment** (from `assistant-en`):

> **User**: Can you explain what a REST API is?
>
> **Assistant**: A REST API (Representational State Transfer) is an architectural style for designing networked applications. It uses HTTP methods like GET, POST, PUT, DELETE to interact with resources identified by URLs.

**Decision**: The user asked a general knowledge question. The answer is publicly available information, not a durable fact specific to this user or project. Skipped.

```json
{
  "schema_version": 1,
  "event_id": "b4d5e6f7-a8b9-0123-defa-b12345678903",
  "library_id": "knowledge",
  "source_agent_id": "assistant-en",
  "conversation_id": "conv-004",
  "reason": "no_durable_fact_detected",
  "committed": false,
  "commit_hash": null,
  "locale": "en",
  "tokens_consumed": 310,
  "snippet": "Can you explain what a REST API is?",
  "timestamp": "2026-04-16T10:03:45Z",
  "duration_ms": 1450,
  "step_spans": [
    {"step": "dedup_check", "ms": 1},
    {"step": "prompt_load", "ms": 13},
    {"step": "llm_call", "ms": 1436}
  ]
}
```

---

### 5. `update_existing_page` -- Correcting a Previously Persisted Fact

**Conversation segment** (from `assistant-en`):

> **User**: Actually, we changed the ARR target. We now need 750k ARR before the Series A, not 500k.
>
> **Assistant**: Understood. I've updated the target: you now need 750k ARR before proceeding with the Series A fundraise in Q3 2027.

**Decision**: The fundraise timeline page already exists with a 500k target. The librarian detects the contradiction and updates the page.

```json
{
  "schema_version": 1,
  "event_id": "c5e6f7a8-b9c0-1234-efab-c12345678904",
  "library_id": "knowledge",
  "source_agent_id": "assistant-en",
  "conversation_id": "conv-005",
  "reason": "update_existing_page",
  "committed": true,
  "commit_hash": "d4e5f6a",
  "locale": "en",
  "tokens_consumed": 410,
  "snippet": "Actually, we changed the ARR target. We now need 750k ARR before the Series A, not 500k.",
  "timestamp": "2026-04-16T10:05:00Z",
  "duration_ms": 2100,
  "step_spans": [
    {"step": "dedup_check", "ms": 2},
    {"step": "prompt_load", "ms": 18},
    {"step": "llm_call", "ms": 2020},
    {"step": "git_commit", "ms": 60}
  ]
}
```

**Library page updated** (`pages/fundraise-timeline.md`):

```markdown
---
source_agent_id: assistant-en
conversation_id: conv-005
timestamp: 2026-04-16T10:05:00Z
snippet: Actually, we changed the ARR target. We now need 750k ARR before the Series A.
---

# Fundraise Timeline

Series A fundraise postponed to Q3 2027. Target: **750k ARR** before raising (updated from 500k).
```

---

### 6. `pii_not_durable` -- Phone Number Flagged as Non-Durable PII

**Conversation segment** (from `assistant-fr`):

> **Utilisateur**: Mon numero de telephone est le 06 12 34 56 78, appelle-moi si besoin.
>
> **Assistant**: Merci pour votre numero. Je le note.

**Decision**: The segment contains a standalone phone number that is not integral to a durable fact. The librarian biases toward skipping non-durable PII.

```json
{
  "schema_version": 1,
  "event_id": "d6f7a8b9-c0d1-2345-fabc-d12345678905",
  "library_id": "knowledge",
  "source_agent_id": "assistant-fr",
  "conversation_id": "conv-006",
  "reason": "pii_not_durable",
  "committed": false,
  "commit_hash": null,
  "locale": "fr",
  "tokens_consumed": 275,
  "snippet": "Mon numero de telephone est le 06 12 34 56 78, appelle-moi si besoin.",
  "timestamp": "2026-04-16T10:06:20Z",
  "duration_ms": 1300,
  "step_spans": [
    {"step": "dedup_check", "ms": 1},
    {"step": "prompt_load", "ms": 16},
    {"step": "llm_call", "ms": 1283}
  ]
}
```

---

### 7. `dedup_merged` -- Restated Fact Merged Into Existing Page

**Conversation segment** (from `assistant-fr`):

> **Utilisateur**: Pour rappel, la levee Series A est prevue au T3 2027 avec un objectif de 500k ARR.
>
> **Assistant**: Oui, c'est bien note. Levee Series A au T3 2027, objectif 500k ARR.

**Decision**: The fundraise timeline page already exists. The segment restates known facts. The librarian merges it (adding a confirmation timestamp) rather than creating a duplicate.

```json
{
  "schema_version": 1,
  "event_id": "e7a8b9c0-d1e2-3456-abcd-e12345678906",
  "library_id": "knowledge",
  "source_agent_id": "assistant-fr",
  "conversation_id": "conv-007",
  "reason": "dedup_merged",
  "committed": true,
  "commit_hash": "f7a8b9c",
  "locale": "fr",
  "tokens_consumed": 380,
  "snippet": "Pour rappel, la levee Series A est prevue au T3 2027 avec un objectif de 500k ARR.",
  "timestamp": "2026-04-16T10:08:00Z",
  "duration_ms": 1950,
  "step_spans": [
    {"step": "dedup_check", "ms": 2},
    {"step": "prompt_load", "ms": 17},
    {"step": "llm_call", "ms": 1870},
    {"step": "git_commit", "ms": 61}
  ]
}
```

---

### 8. `below_locale_confidence` -- Locale Detection Failed (Runtime)

**Conversation segment** (from `assistant-en`):

> **User**: 42
>
> **Assistant**: Could you provide more context? I'm not sure what you're referring to.

**Decision**: The trigram-based locale classifier cannot determine the language of a single number with sufficient confidence (below 0.6 threshold). The librarian skips the event without calling the LLM.

```json
{
  "schema_version": 1,
  "event_id": "f8b9c0d1-e2f3-4567-bcde-f12345678907",
  "library_id": "knowledge",
  "source_agent_id": "assistant-en",
  "conversation_id": "conv-008",
  "reason": "below_locale_confidence",
  "committed": false,
  "commit_hash": null,
  "locale": "",
  "tokens_consumed": 0,
  "snippet": "42",
  "timestamp": "2026-04-16T10:09:10Z",
  "duration_ms": 5,
  "step_spans": [
    {"step": "dedup_check", "ms": 1},
    {"step": "locale_detect", "ms": 4}
  ]
}
```

Note: `tokens_consumed` is 0 because no LLM call was made. The `locale` field is empty because the classifier could not determine the language.

---

### 9. `decision_timeout` -- LLM Call Exceeded Timeout (Runtime)

**Conversation segment** (from `assistant-fr`):

> **Utilisateur**: Voici le compte-rendu complet de notre reunion strategie du 15 avril... *(very long segment)*
>
> **Assistant**: Merci pour ce compte-rendu detaille. Je vais l'analyser.

**Decision**: The conversation segment was extremely long, causing the LLM decision call to exceed the 10-second timeout. The librarian logs the timeout and moves on.

```json
{
  "schema_version": 1,
  "event_id": "a9c0d1e2-f3a4-5678-cdef-a12345678908",
  "library_id": "knowledge",
  "source_agent_id": "assistant-fr",
  "conversation_id": "conv-009",
  "reason": "decision_timeout",
  "committed": false,
  "commit_hash": null,
  "locale": "fr",
  "tokens_consumed": 0,
  "snippet": "Voici le compte-rendu complet de notre reunion strategie du 15 avril...",
  "timestamp": "2026-04-16T10:10:30Z",
  "duration_ms": 10005,
  "step_spans": [
    {"step": "dedup_check", "ms": 1},
    {"step": "prompt_load", "ms": 14},
    {"step": "llm_call", "ms": 9990}
  ]
}
```

Note: `tokens_consumed` is 0 because the LLM call was aborted before completion.

---

### 10. `decision_error` -- Unparseable LLM Response (Runtime)

**Conversation segment** (from `assistant-en`):

> **User**: We should use the new vendor for office supplies starting next month.
>
> **Assistant**: I'll make a note of the vendor change for office supplies.

**Decision**: The LLM returned a malformed response that could not be parsed as valid JSON. The librarian logs the error and skips the event.

```json
{
  "schema_version": 1,
  "event_id": "b0d1e2f3-a4b5-6789-defa-b12345678909",
  "library_id": "knowledge",
  "source_agent_id": "assistant-en",
  "conversation_id": "conv-010",
  "reason": "decision_error",
  "committed": false,
  "commit_hash": null,
  "locale": "en",
  "tokens_consumed": 289,
  "snippet": "We should use the new vendor for office supplies starting next month.",
  "timestamp": "2026-04-16T10:11:45Z",
  "duration_ms": 2200,
  "step_spans": [
    {"step": "dedup_check", "ms": 1},
    {"step": "prompt_load", "ms": 13},
    {"step": "llm_call", "ms": 2186}
  ]
}
```

Note: `tokens_consumed` is non-zero because the LLM did respond, but the response was not valid JSON. The event can be retried if redelivered (dedup cache will not block it since no commit was made).

---

## Summary

After processing these 10 events, the Library contains 3 committed pages:

| Page | Reason | Source |
|---|---|---|
| `pages/fundraise-timeline.md` | `durable_fact_written` then `update_existing_page` | assistant-en |
| `pages/fundraise-timeline.md` | `dedup_merged` (confirmation added) | assistant-fr |

And 7 events were skipped:

| Reason | Source | Why |
|---|---|---|
| `transient_context` | assistant-en | Greeting exchange |
| `contains_secret_pattern` | assistant-en | API key in segment |
| `no_durable_fact_detected` | assistant-en | General knowledge question |
| `pii_not_durable` | assistant-fr | Standalone phone number |
| `below_locale_confidence` | assistant-en | Single number, locale undetectable |
| `decision_timeout` | assistant-fr | LLM call exceeded 10s |
| `decision_error` | assistant-en | Malformed LLM response |

## Viewing Logs in Practice

```bash
# Stream librarian logs in real time
wh logs librarian-knowledge

# Filter for writes only
wh logs librarian-knowledge | jq 'select(.committed == true)'

# Filter for skips with reason
wh logs librarian-knowledge | jq 'select(.committed == false) | {reason, snippet}'

# View git history for the Library
podman exec librarian-knowledge git -C /workspace/.library log --oneline

# Check which agent contributed which page
podman exec librarian-knowledge git -C /workspace/.library log --format="%h %s" --all
```
