---
Update (2025-09-13):
- Fold in native chat + diff/edit UX requirement; these levers must be visible/controllable from TUI chat panes as well as CLI flags.
- Acceptance expanded to include: (a) TUI shows current LLM session params in chat header; (b) From chat, user can toggle confirm_destructive and history_limit for the active session; (c) Reversions can be initiated from a diff view.
- Pointers: vizier-tui/src/chat.rs (chat header + controls), vizier-core/src/history.rs (API), vizier-core/src/config.rs (live session overrides).

---


---
Update (bug fix context): Conversation commits must not include unrelated staged changes.

Behavioral guardrail to codify (acceptance addition):
- When the system records a conversation commit, any pre-existing staged changes unrelated to .vizier are preserved but excluded from that commit, then restored exactly after.

Acceptance:
1) With arbitrary files staged (A/M/D/R), initiating a conversation write creates a commit that touches only .vizier paths; previously staged changes remain staged afterward and are not part of that commit.
2) Rename scenarios and mixed A/M/D staged sets are preserved across the conversation commit.

Pointers: vizier-core/src/auditor.rs (conversation commit flow), vizier-core/src/vcs.rs (stage/unstage/snapshot_staged/restore_staged tests).

Thread: Operation history + reversibility; Narrative contract + drift guardrails.


---


---
Update (2025-09-15): CLI refactor + commit isolation guardrail landed

- CLI refactor: Responsibility for finding the project root moved to vizier-core auditor; CLI now calls auditor::find_project_root(). Keep provider_arg_to_enum in actions.rs for now.
- Commit isolation (behavioral guardrail): Conversation/TODO commits must exclude unrelated staged changes and preserve the staged set exactly after the commit.

Acceptance additions:
1) With arbitrary staged changes (A/M/D/R), a conversation commit modifies only .vizier paths; previously staged changes remain staged and untouched after.
2) Rename scenarios and mixed staged sets are preserved across the conversation commit.

Pointers: vizier-core/src/auditor.rs (conversation commit flow), vizier-core/src/vcs.rs (stage/unstage/snapshot_staged/restore_staged + tests), vizier-cli/src/main.rs (auditor::find_project_root).

Thread: Operation history + reversibility; Narrative contract + drift guardrails.


---

