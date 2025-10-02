# Chat TUI polish: input navigation, loaders, tool-call displays, auto-scroll (Thread: Native Chat + Diff/editor basics; Chat navigation modes)

Problem/Tension
- Input editing lacks expected cursor/navigation affordances; accidental edits occur due to mode ambiguity.
- Loading indicators are sparse and visually noisy; tool calls clutter the timeline.
- The message view does not reliably auto-scroll to the newest messages when new content arrives.

Desired Behavior (Product-level)
- Clear modes: View vs Edit modes with visible state; keyboard navigation consistent and discoverable. Cursor movement, word-jumps, home/end, and history recall work in Edit mode; View mode prevents edits and focuses on navigation.
- Shiny but minimal loaders: a single-line status with spinner for LLM/tool activity, collapsing into a result summary on completion.
- Tool-call cards: compact header (icon, tool name, duration, status), with expandable details (inputs/outputs), and copy-to-clipboard affordance.
- Auto-scroll: On new message or tool result, timeline scrolls to bottom unless the user has manually scrolled up (then show a “Jump to latest” pill).

Acceptance Criteria
1) Input: Arrow keys, Home/End, Ctrl+A/E, Alt/Option+←/→ (or word-jump equivalent) function in Edit mode; none of these alter text in View mode. ESC toggles to View; i toggles to Edit. Visible mode indicator present.
2) History: Up/Down cycle through recent prompts in Edit mode; does not interfere with timeline scrolling in View mode.
3) Loaders: During assistant/tool processing, a single status line with spinner is shown; upon completion it collapses into a 1-line summary with duration and status icon.
4) Tool cards: Successful calls render compact headers; on expand, show structured inputs/outputs with truncation + expand. Failed calls show reason and link to logs.
5) Auto-scroll: New messages cause the viewport to follow the latest unless user is above threshold; then a sticky “Jump to latest” affordance appears; clicking brings you to bottom.

Pointers
- Surfaces: vizier-core/src/chat.rs (render + input handling), vizier-core/src/display.rs, vizier-tui chat panel.
Update [2025-10-02]: Scope limited pending a concrete TUI surface in this repo.
- Defer interactive UI polish (loaders, tool cards, auto-scroll) until a vizier-tui surface exists. Keep product spec as target.
- Near-term: expose headless hooks from vizier-core to emit loader lifecycle events and compact tool-call summaries that the CLI can render. Add a minimal CLI rendering of a single-line spinner and collapse-to-summary on completion.
- Tests: add headless tests for loader lifecycle events and ensure they do not panic; mark rich UI behaviors as blocked.
- Cross-link: Outcome summaries (CLI-first) will subsume some of the post-action summary needs until TUI is available.


---

