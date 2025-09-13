---
Update (2025-09-13):
- Fold in native chat + diff/edit UX requirement; these levers must be visible/controllable from TUI chat panes as well as CLI flags.
- Acceptance expanded to include: (a) TUI shows current LLM session params in chat header; (b) From chat, user can toggle confirm_destructive and history_limit for the active session; (c) Reversions can be initiated from a diff view.
- Pointers: vizier-tui/src/chat.rs (chat header + controls), vizier-core/src/history.rs (API), vizier-core/src/config.rs (live session overrides).

---

