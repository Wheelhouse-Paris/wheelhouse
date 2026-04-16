# Your Library (Read-Only)

This file describes how you -- the agent named **{{agent_name}}** -- use the shared Library. The Library is a git-versioned markdown knowledge base at `.library/` that is maintained by a dedicated librarian agent. **You CANNOT write to the Library -- it is read-only for you.** Any write attempt will fail with a read-only filesystem error.

This schema file is read-only to you. You cannot edit it. Treat its instructions as authoritative.

---

## What the Library contains

The Library holds persistent knowledge about:

- The people you work with (their preferences, context, recurring needs)
- The domain you operate in: **{{domain_description}}**
- Facts, decisions, and commitments that survive past individual conversations
- Knowledge contributed by other agents that share this Library

The Library is maintained by a librarian agent that autonomously decides what to persist from your conversations. You do not need to tell it what to save -- it observes your conversation segments and makes its own decisions.

---

## When to search the Library

At the start of every turn, before composing your response, ask yourself:

> Does this question or topic relate to the domain described above, or to anything that might have been recorded in a prior conversation?

If the answer is **yes**, search the Library. Use `grep` to find relevant pages, then read `index.md` to see how topics are organized. Cite what you find.

If the answer is **no** -- the user is asking about general knowledge, making small talk, or discussing something outside your domain -- **do not search**. Loading Library content into context for unrelated questions wastes tokens and dilutes your focus.

When you are unsure, lean toward searching. A wasted search is cheap. A missed recall is expensive.

---

## How to cite Library content

When you use information from the Library in your response, cite the source page:

1. **Inline citation**: Reference the page path naturally in your response.
   Example: "Based on your preferences noted in your profile, ..."

2. **Explicit citation**: When the user asks where you learned something, provide the page path.
   Example: "I found this in `.library/pages/user-preferences.md`, which was recorded from our conversation on March 15th."

3. **Front-matter attribution**: Each Library page contains YAML front-matter with:
   - `source_agent_id`: which agent's conversation contributed this knowledge
   - `conversation_id`: the conversation where it was captured
   - `timestamp`: when it was recorded

   Use this metadata to tell the user *when* and *how* you learned something.

---

## What you should NOT do

- **Do NOT attempt to write files** to `.library/`. The filesystem is mounted read-only. Any write attempt will fail with an EROFS error.
- **Do NOT include write instructions** in your responses (e.g., "I'll save this to your Library"). The librarian handles all writes autonomously.
- **Do NOT tell the user** to manually edit Library pages. The Library is managed by the librarian agent.
- **Do NOT worry about what gets saved**. The librarian observes your conversations and makes its own persistence decisions. Focus on being helpful.

---

## Answering "What do you know about me?"

When a user asks what you know about them, or asks you to recall prior context:

1. Search the Library for pages related to the user
2. Summarize the relevant findings
3. Cite the source pages
4. Be transparent: say the knowledge comes from the Library, which was built from prior conversations
