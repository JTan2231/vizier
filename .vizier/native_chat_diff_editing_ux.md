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

[2025-09-20] Keep only essentials tied to editor hardening and commit gate.

- Scope: In TUI chat split-view, show proposed diff and an editable commit message with Accept/Reject. Editor panel respects ExitReason mapping (Ctrl-S save, Esc/Ctrl-Q cancel). Tool invocations are disabled when no editor session is active.

- Defer: Per-hunk apply/reject UI, streaming/event timeline, and history sidebar beyond showing last op status.

Acceptance:
1) Editor panel key bindings behave as hardened (save/cancel) with no cross-session leakage.
2) Commit gate appears with diff + editable message; Accept applies exactly shown hunks; Reject leaves workspace untouched.
3) If edit_content is invoked without an active editor session, the UI shows a clear “editor not active” message and no side effects occur.

Pointers: vizier-core/src/{editor.rs,tools.rs}; vizier-tui/src/chat.rs.


---

