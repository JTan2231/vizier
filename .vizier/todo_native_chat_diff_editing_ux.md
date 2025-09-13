# Native Chat + Diff/Editing UX

Context
- Users want a native chat interface that can: (1) host real-time conversations with the model, (2) show diffs created by the model or by local VCS, and (3) allow editing with the model as a partner in real-time.
- Longer-term desire: expose what the model is doing while requests are processed; implies streaming/networking visibility and instrumentation.
- This threads into existing themes: control levers, history/revert, and contract/guardrails.

Promise vs. current behavior
- Current TUI has a chat module but lacks integrated diff views and collaborative editing affordances.
- No explicit streaming visibility into in-flight model/tool activity beyond final outputs.

Scope (Phase 1)
- Add split-view chat pane: left = chat, right = contextual panel that can switch between: (a) diff viewer, (b) file editor, (c) operation history.
- Allow the model to propose patches; user can preview diffs and apply/revert from the diff panel.
- Provide minimal collaborative editing: cursor + selection sync to the model context and "apply suggestion" actions.

Acceptance criteria (Phase 1)
1) From chat, when the model proposes a file change, a diff appears in the side panel; user can accept or reject per-file and per-hunk.
2) User can open an arbitrary file in the side panel and request suggestions; suggestions return as patch sets that are previewable and reversible via History.
3) The chat header shows current LLM session settings (model, temperature, history_limit, confirm gate), and changes there affect subsequent requests.
4) All applied changes are recorded as Operations with patches; Revert from history restores prior state without stray files.
5) Streaming outputs: token stream visible in chat; panel shows "in progress" markers for tools/patch generation.

Scope (Phase 2 – Networking/Instrumentation)
- Instrument a "live activity" stream that exposes model/tool events (prompt sent, token delta, tool call issued, file scan/diffing progress) to the UI.
- Provide a compact timeline view in the side panel to visualize the request lifecycle.

Acceptance criteria (Phase 2)
A) Chat shows a live activity feed during requests, including timestamps and event types. Errors are surfaced inline with retry affordances.
B) Network/stream transport is abstracted behind a stable interface; TUI consumes SSE/WebSocket-like stream without assuming a specific library.
C) Privacy/safety: redact secrets in-stream; respect non_interactive_mode and contract strictness.

Pointers
- TUI: vizier-tui/src/chat.rs (existing chat), new side panel module(s).
- Core: vizier-core history API (planned), config live overrides, streaming observer hooks (observer.rs, tools.rs).
- CLI: flags for enabling streaming contract/logging if TUI is not used.

Implementation notes (allowed: safety/correctness)
- Diff apply/revert must be atomic; falling back to VCS is acceptable if patch apply fails.
- Streaming must tolerate network drops; UI should resume or degrade gracefully to buffered final output.