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

