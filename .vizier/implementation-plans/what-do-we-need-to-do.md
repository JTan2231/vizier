## Overview
Operators relying on Codex through Vizier’s chat TUI only see opaque `turn.started` / `item.completed` lines while Codex edits the repo, so they cannot tell which files or tools are being exercised. To keep the Codex-integrated posture credible (snapshot: Codex now edits the tree directly, CLI-first IO contract) we need a richer progress feed that surfaces what Codex is doing in real time while respecting the stdout/stderr guardrails and Pending Commit gates. This plan adds structured progress translation in `vizier-core` so both the CLI spinners and the chat TUI can expose meaningful, audit-friendly status without waiting for the Outcome epilogue.

## Execution Plan
1. **Map and normalize Codex event semantics**
   - Instrument `vizier-core/src/codex.rs` to capture representative `CodexEvent` payloads (turn/item/tool/run events) in debug logs or a temporary trace so we know which fields (e.g., `label`, `message`, `tool_name`, `path`) are stable.
   - Design a typed `ProgressUpdate` struct that classifies events into a small set of phases (thinking, editing file, running command, committing, etc.) with optional metadata (target path, tool, elapsed) and severity.
   - Replace the string-only `summarize_event` helper with a dispatcher that converts every Codex event into one of these updates, falling back to a generic description when an event is unknown. Keep the raw payload attached for future diagnostics.
   - Document the mapping in-code (doc comments) so future Codex event changes are easy to reconcile.

2. **Upgrade the progress plumbing for CLI callers**
   - Extend `ProgressHook` so `Display` mode receives `ProgressUpdate` structs instead of bare strings; teach `vizier-core/src/display.rs` to render the latest update line (respecting `-q/-v/-vv`, `--progress`, and non-TTY rules from the stdout/stderr contract thread) while still allowing the spinner.
   - Ensure the CLI spinner/line output shows the classified message (e.g., “Applying patch to vizier-core/src/chat.rs”) and accumulates the most recent update so operators running `vizier ask` or `vizier save` get the same fidelity as the TUI.
   - When progress is suppressed (quiet/non-TTY), store the most recent update so it can be echoed in the Outcome summary for auditability without violating the IO contract.

3. **Expose a dedicated progress pane inside the chat TUI**
   - Stop injecting progress updates as fake assistant messages; instead, keep a bounded ring buffer of `ProgressUpdate`s on the `Chat` struct so they don’t pollute the conversation history.
   - Redraw the TUI layout with a slim status pane (top or bottom) that lists the latest N updates with timestamps/spinner icons, so operators can watch Codex work (“Collecting snapshot”, “Running apply_patch”, “Waiting on tests”). Make the pane collapsible to avoid crowding small terminals.
   - When Codex finishes, clear the active-progress indicators but retain the last completed update so the user can see the final action performed before the assistant reply arrives.
   - Ensure the pane honors color/ANSI toggles (still no ANSI in non-TTY) and gracefully falls back to text-only logging if Ratatui is not in use (e.g., future protocol mode).

4. **Wire auditor + docs hooks**
   - Include the most recent classified progress line in the Auditor facts so future Outcome summaries (thread: Outcome summaries + Agent orchestration) can cite what Codex was doing if the run aborts mid-stream.
   - Update README (Chat section) or AGENTS.md blurb to mention the richer progress view so operators know how to interpret the pane and where to look when running in Codex mode.

## Risks & Unknowns
- Codex event schemas may change without notice; we need to maintain an allowlist and a safe fallback so Vizier doesn’t panic on new event types.
- Streaming too many detailed updates could starve the UI or flood stderr; throttling/batching policy must be tuned so we keep terminals responsive.
- TUI layout changes must not break existing keybindings or accessibility; careful sizing logic is needed for narrow terminals.
- We need confidence that surfacing paths/tool names does not leak sensitive info when recorded in logs or session artifacts; confirm with stakeholders before persisting everything.

## Testing & Verification
- Unit tests for the new `ProgressUpdate` classifier in `vizier-core/src/codex.rs` that feed fixture JSON events (turn/item/tool/etc.) and assert the human-facing strings/metadata match expectations and unknown events fall back gracefully.
- CLI integration test (mock LLM feature) that simulates a Codex run emitting a scripted event sequence, then asserts the captured CLI stderr/stdout matches the configured verbosity/progress levers and never emits ANSI when stdout is not a TTY.
- Ratatui-focused test (or headless snapshot test) that instantiates the `Chat` struct with synthetic progress updates and verifies the progress pane renders the bounded buffer without altering the chat transcript or causing panics when toggling visibility.
- Regression test ensuring the Auditor captures the last progress update so that if we simulate an error mid-run, the Outcome summary mentions what Codex was doing.

## Notes
- Touches `vizier-core/src/codex.rs`, `vizier-core/src/display.rs`, `vizier-core/src/chat.rs`, and `vizier-core/src/auditor.rs`; coordinate changes so stdout/stderr contract and pending Outcome-summary work stay aligned.
- Narrative impact: establishes observable Codex progress so operators trust the default-action posture while Codex edits the tree.
