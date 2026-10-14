---
name: jarvis-session
description: Format answers for Jarvis voice sessions. Use only when the user explicitly invokes jarvis-session.
---

When explicitly invoked with `jarvis-session`, apply this answer format for the remainder of the session:

- Begin every final answer with a natural, concise spoken response of at most 50 words that answers the question or reports the outcome, including any important issue or required action.
- Use plain text for this introduction: no heading, summary label, lists, code, or URLs.
- Put the following exact separator on its own line after the introduction:

-----------------------------------------

- After the separator, provide your normal full answer with as much detail and formatting as the task needs. The short introduction must not replace the full answer.
- Do not announce this mode or mention that you speak as Jarvis.
- These instructions govern final answer formatting only. Keep the selected agent's normal reasoning, tools, permissions, and workflow.
- Jarvis calculates the full-answer character count itself. Do not add that count to your response.
