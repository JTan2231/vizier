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

---
Update (2025-10-02): Clarified DAP as ACTIVE by default across CLI; users can opt out per-turn with phrases like "discuss-only"/"no-op". Acceptance: CLI epilogue prints a one-line Outcome listing created/updated items when DAP acts. Cross-links: Outcome summaries TODO; Integration tests coverage TODO.


---

Enable Default-Action Posture (DAP) by default with per-turn opt-out and aligned Outcome epilogue.
Description:
- When a user message implies change (feature, bug, prioritization, acceptance), the assistant updates the Snapshot and/or creates TODOs in the same turn without extra prompting. Users can suppress action with opt-out phrases (e.g., “no-op”, “discuss-only”) scoped to that turn.
- All changes respect commit gates and isolation; only .vizier artifacts (snapshot, todos) are modified. A concise Outcome line summarizes what changed and mirrors Auditor/VCS facts. (thread: DAP)

Acceptance Criteria:
- Default action: Given a directive like “search feels slow; prioritize fixes”, the assistant:
  - Updates the Snapshot narrative and creates at least one TODO that cross-links to relevant thread(s); no duplicate threads are created.
  - Assistant final includes a one-line Outcome that lists created/updated items and counts; CLI prints the same epilogue; both facts match Auditor/VCS.
- Opt-out: If the user prefixes “no-op:” or “discuss-only:”, the assistant returns analysis only; no writes occur to .vizier. Outcome states “No changes (no-op requested).”
- Ambiguity guardrail: For clearly non-directive/ambiguous chit-chat (e.g., “how are you?”), no changes are made. Outcome states “No changes (no directive detected).”
- Gates/isolation: If a conversation/pending-commit gate is active, Outcome reflects gate state (open/accepted/rejected/skipped) and why; no code changes are made by DAP beyond .vizier unless an existing gate policy permits.
- CLI surface: A one-line Outcome epilogue appears after DAP actions; hidden with --quiet; when --json or protocol mode, outcome.v1 JSON is emitted on stdout with audited counts and gate state.
- Consistency: Assistant final message, CLI epilogue, and JSON (when requested) agree exactly on A/M/D/R counts and item lists (bounded).
- Tests: Cover (a) default action with new TODO + snapshot update, (b) opt-out no-op, (c) ambiguity no-op, (d) gate-open pending state, (e) rejection path, (f) protocol/--json JSON shape validation, and (g) duplicate-thread prevention.

Pointers:
- Policy/dispatch around user message classification; CLI ask epilogue (vizier-cli/src/actions.rs); .vizier/.snapshot storage; Auditor/VCS facts as Outcome source.

Implementation Notes (scope/safety):
- CLI-first: TUI indicators are deferred until a UI surface exists; ensure no ANSI in non-TTY and adhere to stdout/stderr contract. Compute Outcome from Auditor/VCS after writes and before exit; never infer from model text.Enable Default-Action Posture (DAP) by default with per-turn opt-out and aligned Outcome epilogue.
By default, when a user message implies a change (feature, bug, prioritization, acceptance), apply Snapshot and/or TODO updates in the same turn without extra prompting. Users can suppress action per-turn with opt-out phrases (e.g., “no-op: …”, “discuss-only: …”). All writes are confined to .vizier artifacts and summarized via the Outcome component sourced from Auditor/VCS facts. (thread: DAP; cross: outcome-summaries, stdout-stderr-contract, integration-tests)

Acceptance Criteria:
- Default action:
  - Given a directive like “search feels slow; prioritize fixes,” the assistant updates the Snapshot narrative and creates at least one cross-linked TODO advancing an existing thread (no duplicates) or, if necessary, starts one new thread with stable IDs and links.
  - Assistant final includes a one-line Outcome listing created/updated items and counts; the CLI prints the same epilogue; both match Auditor/VCS facts.
- Opt-out:
  - If the user prefixes “no-op:” or “discuss-only:”, the assistant returns analysis only; no writes occur to .vizier.
  - Outcome states “No changes (no-op requested).”
- Ambiguity guardrail:
  - For clearly non-directive/chit-chat inputs (e.g., “how are you?”), no changes are made.
  - Outcome states “No changes (no directive detected).”
- Gates and isolation:
  - DAP only writes within .vizier (snapshot, todos). No code changes are produced by DAP.
  - If a Pending Commit gate pertains to conversation artifacts, Outcome reflects gate state (open/accepted/rejected/skipped) and why (auto_commit/non_interactive/no changes).
- CLI surface:
  - A one-line Outcome epilogue appears after DAP actions; hidden with --quiet.
  - With --json or in protocol mode, outcome.v1 JSON is emitted on stdout with audited counts, bounded item lists, and gate state; no ANSI escapes in non-TTY.
- Consistency:
  - Assistant final, CLI epilogue, and JSON (when requested) agree exactly on A/M/D/R counts and item lists (bounded).
- Tests:
  - Cover (a) default action with new TODO + snapshot update, (b) opt-out no-op, (c) ambiguity no-op, (d) gate-open pending state, (e) rejection path, (f) protocol/--json JSON shape validation and no ANSI in non-TTY, and (g) duplicate-thread prevention.

Pointers:
- Policy/dispatch for directive detection; CLI ask epilogue (vizier-cli/src/actions.rs); .vizier/.snapshot storage; Auditor/VCS facts as Outcome source.

Implementation Notes (safety/correctness):
- CLI-first scope; defer any TUI indicators until a UI surface exists.
- Compute Outcome from Auditor/VCS after writes and before exit; never infer from model text.
- Honor stdout/stderr and mode-split contracts: human epilogue to stdout by default; structured outcome.v1 JSON when requested; stderr limited to errors/progress per verbosity.