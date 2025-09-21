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

Update (2025-09-20, later): Chat returns with styling

- Evidence: Author note indicates “including the chat back with some added styling.” Treat as reintroducing the TUI chat surface with cosmetic improvements.

Implications (scope stays the same functionally):
- Keep Phase 1 scope to split-view with diff + editable commit message and Accept/Reject. Styling changes must not alter keybindings or gate semantics.
- Ensure chat header can display current LLM session settings; visual polish is allowed but behavior must satisfy acceptance.

Acceptance deltas:
4) Visual: Chat pane renders with consistent theming (header, borders, focus states) without breaking keyboard shortcuts listed below.
5) Keyboard shortcuts remain: Accept, Reject, and editor Save (Ctrl-S) / Cancel (Esc/Ctrl-Q) operate as previously specified.

Pointers: vizier-tui/src/chat.rs (reintroduced); vizier-core/src/{editor.rs,tools.rs}.


---

Update (2025-09-21): Long-message rendering fix + basic scrolling landed

- Evidence: Author note indicates a rendering bug with long, multiline messages was fixed and basic scrolling was added in the TUI chat surface (vizier-core/src/chat.rs).

Implications for UX & tests:
- Chat must correctly wrap and render multiline content without overlapping UI elements or truncation.
- Scrolling affordance should allow navigating older chat content without disrupting focus or editor/diff panels when present.

Acceptance additions:
3.a) Given a conversation containing messages exceeding the visible pane height, the chat view allows scrolling back to reveal earlier content; the scroll position is bounded (no blank space past first/last message) and the layout remains stable while scrolling.
3.b) Long lines wrap within the chat pane width; no text spills into borders or adjacent panels, and no content is lost when resizing the terminal smaller then larger.

Pointers: vizier-core/src/chat.rs (rendering and input handling).

---

