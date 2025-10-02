# Minimal‑invasive terminal chat UI + renderer‑neutral surface

Thread: Terminal‑first minimal TUI + renderer‑neutral events (NEW). Cross‑links: Outcome summaries; Native chat navigation; Chat TUI polish; Architectural invariants (surfaces).
Depends on: Snapshot (Running Snapshot — updated), Code State (CLI + vizier‑core only; no vizier‑tui crate).

Tension
- Fullscreen TUIs feel heavy and invasive; we want the chat to “just write to the terminal,” preserving scrollback and playing nicely with shells, tmux, SSH, and piping. Simultaneously, we foresee non‑terminal renderers (web/other). The current path lacks a renderer‑neutral contract and risks coupling UI behavior to one surface.

Desired behavior (product level)
- Terminal‑first, minimally invasive output by default:
  - No alternate screen/fullscreen redraws; avoid constant full‑frame painting.
  - Line‑oriented streaming that appends/updates conservatively; preserve scrollback; do not hijack the cursor when not a TTY.
  - Works cleanly when piped (no ANSI control sequences in non‑TTY mode) and in CI.
- Progressive feedback:
  - Token/line streaming with subtle single‑line status while tools/LLM run; collapses into a concise Outcome line on completion.
- Visual restraint and clarity:
  - ASCII/Unicode minimalism; clear separators; consistent timestamps/roles; color optional with safe fallbacks.
- Renderer‑neutral factoring:
  - Core emits a stable stream of render events (message_start, token, message_end, tool_begin, tool_end, status, outcome) that surfaces (CLI, future TUI, web) can consume.
  - CLI renders human text by default; a `--json-stream` mode exposes the same events for web/other subscribers.
- Interactivity is opt‑in:
  - If TTY and user enables interactive mode, lightweight key handling is allowed (e.g., pause/resume stream, open details). Otherwise, non‑interactive text output only.

Acceptance criteria
1) Default run is line‑oriented and does not enter alt‑screen/fullscreen. In non‑TTY contexts (pipes/redirects), output contains no control sequences and remains readable.
2) Streaming: While generating, a single status line with spinner appears only on TTY; on completion, it collapses to a 1‑line Outcome summary consistent with Auditor/VCS facts. No flicker or scroll‑jumps in standard terminals.
3) Outcome: The final assistant turn and CLI epilogue include the same concise Outcome block. When no changes occur, explicitly state so.
4) Renderer‑neutral events: vizier‑core exposes a documented event contract (versioned) that includes message/tool/status/outcome lifecycle. CLI renders these; `--json-stream` emits them as newline‑delimited JSON.
5) Safety and portability: Behavior is stable over SSH, inside tmux, with narrow widths; color can be disabled (`NO_COLOR` respected). Piping to a file yields clean logs.
6) Tests: Cover TTY vs non‑TTY behavior gates; ensure no ANSI escapes in non‑TTY; verify presence/format of the Outcome line; snapshot tests for `--json-stream` event schema.

Pointers (anchors)
- vizier-core/src/display.rs (render hooks), vizier-core/src/chat.rs (chat lifecycle), vizier-core/src/auditor.rs (facts → outcome), vizier-cli/src/actions.rs and main.rs (flags, epilogue), tests/ (integration around TTY detection and JSON stream).

Notes
- Do not choose libraries or prescribe alt‑screen mechanics; keep the implementation open. The core decision is the product contract: terminal‑first minimal output and a renderer‑neutral event stream to unlock web/other surfaces later.
