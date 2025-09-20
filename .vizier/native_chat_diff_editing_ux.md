Update (2025-09-20): Editor lifecycle + exit semantics tightened

- Evidence: Changes in vizier-core/src/editor.rs and vizier-core/src/tools.rs now use an ExitReason enum in run_editor() and tie the editor message channel (SENDER) lifetime to a single editor session. The handle is cleared when the editor exits, preventing cross-session leakage and accidental sends to a stale channel.

Implications for TUI chat + diff/editor UX:
- The in-chat editor panel must assume the tool channel exists only while an editor session is active. UI should disable LLM-driven edit_content tool invocations when no session is active and surface a clear error if attempted.
- Exit mapping is explicit: Esc or Ctrl-Q → Cancel; Ctrl-S → Save. Acceptance must verify these keys produce the expected outcomes in the split-view editor.

Acceptance additions:
1) When the editor panel is closed, attempts to trigger edit_content via tools return a user-visible “editor not active” message with no side-effects.
2) During an active editor session, Ctrl-S saves and returns content to caller; Esc/Ctrl-Q cancel and discard changes; no stray messages are delivered to subsequent sessions.

Pointers: vizier-core/src/editor.rs; vizier-core/src/tools.rs; vizier-tui/src/chat.rs (editor panel state + tool gating).

---

