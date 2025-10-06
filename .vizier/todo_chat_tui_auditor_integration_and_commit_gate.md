# Chat TUI: Auditor integration + commit gate cohesion (Thread: Commit isolation + gates; Native Chat)

Problem/Tension
- Actions executed from Chat TUI bypass the Auditor’s accountability trail and the automatic commit mechanism of `ask`. This breaks the promise that AI-led changes are auditable and gated.
- Interface is inconsistent: tool calls/loaders are noisy; navigation is clumsy; message list doesn’t reliably auto-scroll to the latest entry.

Desired Behavior (Product-level)
- Every tool/action initiated from Chat TUI is funneled through the Auditor with a uniform audit record (who/what/why/when) and participates in the Pending Commit gate, identical to `ask`.
- When auto_commit=false or confirm_destructive=true, changes pause at the Pending Commit gate with an in-TUI affordance to accept/reject; acceptance produces a conventional commit authored as “vizier-assistant” with the chat context captured in the commit body.
- Chat timeline shows compact, readable entries for tool invocations and their outcomes, including success/failure summaries and expandable details.

Acceptance Criteria
1) Auditor Path: Triggering a code-modifying action from the Chat TUI produces an Auditor entry and a corresponding pending change set that is visible to the VCS layer without committing by default.
2) Gate Parity: With auto_commit=false, after an action the TUI shows a Pending Commit banner with options [Accept Commit], [View Diff], [Discard]; selecting Accept creates a commit; Discard rolls back all staged changes from that action.
3) Commit Body: The commit includes: (a) the assistant message that led to the action, (b) a short tool summary line, (c) a reference to session id/message id.
4) Isolation: Pre-existing staged changes remain unchanged (A/M/D/R parity) after running Chat actions; only the new changes are gated.
5) Failure Surfacing: If an Auditor step fails, the chat renders a concise error chip with an expandable panel for logs; no partial commits occur.

Pointers
- Surfaces: vizier-core::{auditor.rs, vcs.rs, chat.rs}; vizier-cli/src/actions.rs; TUI chat panel rendering.

---
Progress update (current):
- Chat path now routes through the Auditor. Auditor can observe/chat events and produce facts for post-action summaries.

Remaining scope to close this thread:
- Ensure Pending Commit gate engages consistently for chat-initiated changes (respect confirm_destructive and auto_commit settings). 
- Emit a unified Outcome epilogue (human + outcome.v1 JSON) sourced from Auditor facts at the end of each chat operation.
- Wire session persistence hooks so each chat session writes a session.json record with the audited facts and workflow metadata.

Acceptance criteria additions:
- When chat applies changes, the Auditor shows A/M/D/R counts and the CLI prints a one-line Outcome and JSON (when --json) that matches these facts.
- With auto_commit=false, chat changes remain pending until explicitly saved; with confirm_destructive=true, destructive diffs require confirmation.
- After a chat completes, a session JSON artifact exists on disk and validates against the session schema (see session logging TODO).


---

Route chat-initiated changes through Auditor and the Pending Commit gate; emit unified Outcome; persist session (CLI-first).
Description:
- All chat-initiated code changes are funneled through the Auditor and participate in the Pending Commit gate, mirroring `ask`. No direct commits unless policy allows (auto_commit) and destructive confirmations are honored (confirm_destructive). Pre-existing staged A/M/D/R remain untouched. On accept, create a conventional commit authored as “vizier-assistant” with brief chat/tool context in the message; on reject, restore the pre-op tree. Assistant final and CLI epilogue present a single, factual Outcome sourced from Auditor/VCS facts. TUI affordances are deferred until a UI surface exists; expose gate state and facts via CLI and events. (thread: Commit isolation + gates; cross: Outcome summaries, session-logging; snapshot: Running Snapshot — updated)

Acceptance Criteria:
- Auditor path:
  - After a chat operation that produces code changes, Auditor facts include A/M/D/R counts and changed paths; a pending gate state is created if auto_commit=false or confirm_destructive=true.
  - If no changes were produced, Outcome explicitly states “No code changes were created” and JSON includes {diff:false}.
- Gate behavior and isolation:
  - With auto_commit=false, chat changes remain pending until explicitly accepted; acceptance creates a commit; rejection discards the pending changes. Pre-existing staged changes are preserved exactly.
  - With confirm_destructive=true and a destructive diff, Outcome reports “blocked: confirmation required” and no commit is created.
- Outcome delivery:
  - CLI prints a concise epilogue on stdout after each chat operation; hidden with --quiet. With --json, stdout emits a single outcome.v1 object; assistant final mirrors the same facts. Fields include {action, elapsed_ms, changes, commits, gates:{state,reason}, branch?, pr_url?, next_steps?}.
  - Non-TTY never emits ANSI; stderr only carries diagnostics per -v/-vv.
- Commit body:
  - When accepted, the commit message includes: the initiating assistant message excerpt, a one-line tool summary, and references to session_id/message_id.
- Session logging:
  - After each chat operation, a session JSON artifact exists under .vizier/sessions/<id>/ and validates against the MVP schema; it records Auditor facts, gate state transitions, and outcome identifiers. Outcome (human and JSON) includes the session file path.
- Failure surfacing:
  - If an Auditor/VCS step fails, no partial commits occur; Outcome summarizes the error with a non-zero exit code in protocol/JSON paths.
- Tests:
  - Cover: (a) no changes, (b) pending gate open, (c) auto-commit true, (d) destructive confirmation required, (e) accept → commit created with correct author/message, (f) reject → workspace restored, (g) failure path with no partial writes, (h) preservation of pre-existing staged A/M/D/R, (i) non-TTY emits no ANSI and stdout carries Outcome, (j) outcome.v1 JSON shape and facts alignment.

Pointers:
- vizier-core/{auditor.rs, vcs.rs, chat.rs} (audit + gate + chat flow), vizier-cli/src/actions.rs (epilogue and flags), session logging hooks (writer/validation).

Implementation Notes (safety/correctness):
- Gate transitions must be atomic: accept/write/record or restore/no-op; never mix pending changes with pre-existing staged content. Compute and emit Outcome after all writes and before process exit.