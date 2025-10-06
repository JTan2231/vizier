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


---
Rendering constraints update (2025-10-02)
- Adopt the terminal-first, minimal-invasive philosophy: no alt-screen/fullscreen redraws; prefer line-oriented updates. Loaders are single-line and collapse to Outcome. Any future chat panel must layer on top of the renderer-neutral event stream and honor non-TTY fallbacks. Rich cards/auto-scroll are deferred until a UI surface exists; CLI-first rendering should remain readable without control sequences when piped.
- Cross-link: See TODO “minimal_invasive_tui_and_renderer_neutral_surface” for the product contract and acceptance criteria that this polish must respect.


---


---
Update (2025-10-06): Add ergonomics thread for scrolling in chat UI.

- Tension: Users report difficulty with scrolling in the chat UI; long conversations and tool outputs overflow the viewport without clear navigation affordances.
- Product requirement: Ensure smooth, predictable scrolling in chat surfaces (CLI-rendered chat log and any future TUI). Support keyboard-based paging (PageUp/PageDown, Home/End), incremental line scroll (Up/Down), and search-jump anchors for tool result blocks. Maintain renderer-neutral contract: no fullscreen control codes in non-TTY; in TTY, use minimal cursor movement compatible with tmux/SSH.
- Acceptance criteria:
  - When chat history exceeds viewport, users can page through without losing focus context; current selection is visibly indicated.
  - Scroll actions never emit ANSI in non-TTY contexts and degrade to plain pagination prompts if needed.
  - Large tool outputs are collapsible/expandable; collapsed blocks maintain a one-line summary with length and status.
  - A status line shows scroll position (e.g., "42/120 lines").
  - Works under tmux and when terminal height changes mid-session.
- Anchors: vizier-core::display, vizier-core::chat; potential future vizier-tui (deferred), CLI renderer in vizier-cli/src/actions.rs.
- Cross-links: Terminal-minimal TUI thread; Renderer-neutral event stream; Outcome summaries for tool blocks.


---

