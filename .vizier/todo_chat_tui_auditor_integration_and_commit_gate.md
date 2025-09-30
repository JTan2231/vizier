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
