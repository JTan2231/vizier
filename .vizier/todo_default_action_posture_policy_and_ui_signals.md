# Default-Action Posture (DAP): assistant takes actions without explicit instruction

Thread: Default-Action Posture (DAP) — see Snapshot “NEW thread: DAP” (Running Snapshot — updated)
Related threads: Outcome summaries across interfaces; Integration tests; Narrative timelines.

Problem
Users must currently append phrases like “please take an action” to get Vizier to update TODOs or the Snapshot. This contradicts the story-editor ethos and creates friction.

Desired behavior (Product-level)
- By default, any user directive that implies change (e.g., feature request, bug report, prioritization, acceptance feedback) results in concrete TODOs and/or Snapshot updates in the same turn, without extra prompting.
- Users can suppress action by explicitly opting out in the message (e.g., prefix: "no-op", "discuss-only"), or by toggling a UI control that scopes the next message to discussion.
- After any such action, present a concise Outcome line showing what changed (e.g., “Updated snapshot; added 1 TODO”).
- Respect commit gates and isolation: changes occur within .vizier (snapshot, todos), and are summarized through the Auditor/Outcome component.

Acceptance criteria
1) Assistant processes a directive like “search feels slow; prioritize fixes” and, without further instruction, updates the snapshot narrative and creates TODOs that cross-link to relevant threads.
2) When the user prefixes “no-op: …”, the assistant replies with analysis but makes no changes; the Outcome shows “No changes (no-op requested)”.
3) TUI and CLI display a clear indicator when DAP is active for the current message; indicator clears on no-op.
4) Outcome summaries list created/updated files and counts, consistent between Assistant final message and Auditor facts.
5) Integration tests cover default action, opt-out path, and thread cross-linking, ensuring no duplicate threads.

Pointers
- Assistant policy layer; Chat TUI status line and message header; CLI `ask` epilogue; .vizier/.snapshot and TODO storage.

Trade space / notes
- Provide gentle guardrails to avoid changes on ambiguous chit-chat (e.g., questions without directives). Favor minimal changes with strong cross-links to existing threads. Implement ambiguity detection with a fallback “ask to confirm” only when confidence is low.
