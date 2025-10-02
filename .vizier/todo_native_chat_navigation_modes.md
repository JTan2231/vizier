Thread: Chat navigation modes (snapshot: “Chat navigation modes (new, active)”) — evolve the Native Chat + Diff/editor basics arc by adding modal navigation to prevent accidental edits while improving movement through chat/diff content.

Goal
- Introduce View vs Edit modes for the chat/diff pane with clear affordances and discoverable controls. Default to View mode; require explicit switch to Edit mode for text changes.

Behavior/UX
- Mode indicator visible in status/header (e.g., `[VIEW]` or `[EDIT]`).
- View mode: cursor moves and scrolling work across the chat transcript and diff panes; typing does not modify content; navigation keys move focus/selection; Enter activates focused affordance (e.g., open message details, toggle pane focus).
- Edit mode: typing inserts/deletes in the focused editable field (e.g., commit message or reply input); navigation keys behave as text editing commands within the field.
- Switching: provide default vim-esque bindings (h/j/k/l for movement; `i` to enter Edit; `Esc` to return to View) plus arrow keys/PageUp/PageDown. Must be discoverable via a help overlay (e.g., `?`).
- Remapping: Provide a minimal keymap layer so bindings can be customized in config later; for now, keep a static map but structure input handling to support future remaps.
- Safety: In View mode, accidental keystrokes do not alter any buffers. Attempting to edit shows a subtle hint to switch modes.

Acceptance criteria
1) Default entry is View mode; visible indicator shows the mode.
2) In View mode, alphanumeric keys do not change text; h/j/k/l and arrows scroll/move; Esc is idempotent.
3) Pressing `i` (or an explicit Edit key) enters Edit mode focused on the reply input or commit message field; mode indicator updates.
4) In Edit mode, text entry works; Esc returns to View without losing typed content.
5) Help overlay (`?`) lists current bindings for both modes.
6) Tests cover: mode switch, edit guard in View mode, and scroll bounds unaffected by random typing.

Pointers
- vizier-core/src/chat.rs (input handling for chat/diff panes)
- vizier-core/src/display.rs (status/header)
- vizier-core/src/config.rs (future: keymap remapping hook)Update [2025-10-02]: Defer UI implementation until a concrete TUI surface/crate exists in this repo.
- Keep the product/acceptance spec intact as the target behavior.
- Current scope: ensure core display/input hooks in vizier-core can expose a mode flag and accept mode switch events if/when a TUI consumer wires them. Do not mandate keymaps yet; document intent in config schema comments.
- Testing: limit to headless hooks (e.g., simulated mode flag transitions) to avoid coupling to a missing TUI. Mark full interactive tests as blocked by UI availability.
- Cross-link: Snapshot notes absence of vizier-tui crate; related work is deferred. Tie to Outcome/DAP threads only for discoverability text in CLI headers (optional).


---


---
Rendering constraints (2025-10-02)
- Modes must layer atop the renderer-neutral event stream and honor terminal-minimal behavior: avoid fullscreen redraws; interactivity is opt-in and only when TTY.
- Until a TUI surface exists, expose headless mode flags and transitions; defer interactive keymaps; keep CLI output strictly line-oriented.
- Cross-link: Thread “Terminal-first minimal TUI + renderer-neutral events” and TODO “minimal_invasive_tui_and_renderer_neutral_surface”.


---

