## Overview
Codex-based runs only surface opaque `turn.started/turn.completed` messages in both the CLI progress feed and the chat TUI, so operators cannot see which tools, commands, or files Codex is touching. The wire backend still emits descriptive status lines, so Codex currently feels blind despite the repo-level editing posture described in the snapshot (“Codex now handles the heavy agent work directly”). We need to translate Codex’s JSON event stream into human-readable progress, thread that insight through the CLI/TUI surfaces, and make the trace inspectable so operators can trust Codex-era workflows.

## Execution Plan
1. **Capture and classify Codex events**
   - Extend `vizier-core/src/codex.rs` to retain the raw `CodexEvent` payloads (they already arrive via `run_exec`) and add a small parser that recognizes the stable event families Codex emits (turn/tool/command/apply_patch/file write, etc.). When an event exposes structured fields such as `tool`, `command`, `path`, or `status`, normalize them into a `ProgressUpdate` struct with `{phase, summary, detail, severity}` and keep the original payload available for debugging.
   - Add a debug/trace hook (only active at `-vv` or via a config toggle) that can dump the raw event JSON to the log so we can validate the parser whenever Codex changes its schema. This also gives operators a safety valve to see the undigested stream when needed.
   - Acceptance: triggering a Codex-backed `vizier ask` captures a sequence of typed updates (not just strings), the parser never panics on unrecognized events, and a `-vv` run can show the raw JSON for comparison.

2. **Surface descriptive progress in CLI flows**
   - Update `ProgressHook` so it carries the new `ProgressUpdate` struct; teach the `display::call_with_status` spinner/line renderer (TTY and non-TTY) plus `spawn_plain_progress_logger` in `vizier-cli/src/actions.rs` to format the summary (e.g., “shell.run: rg --files” or “apply_patch → vizier-core/src/chat.rs”). Respect the IO contract thread: quiet mode still suppresses chatter, non-TTY never shows ANSI, and we keep only the latest summary when progress is hidden.
   - After Codex finishes, emit the last meaningful update alongside the Outcome lines so even commands run with `--quiet` can see what Codex was doing if something fails.
   - Acceptance: running `vizier ask --backend codex` shows per-tool progress (matching what the wire backend prints), adheres to verbosity flags, and the Outcome mentions the last update if an error occurs mid-run.

3. **Add a proper progress pane inside the chat TUI (and keep it out of the transcript)**
   - Stop injecting progress lines as faux assistant messages in `vizier-core/src/chat.rs`; instead, maintain a bounded buffer of `ProgressUpdate`s on the `Chat` struct and render them in a dedicated pane (stacked above or beside the transcript) that auto-scrolls as events arrive. Include minimal timestamps/spinner icons so users can monitor Codex while it edits.
   - Make the pane collapsible/toggleable for small terminals and ensure it honors color/ANSI settings (no ANSI when stdout/stderr aren’t TTYs). Preserve the last completed update after Codex replies so the user knows the final action executed.
   - Acceptance: running `vizier chat` with Codex inside a TUI session shows tool-level progress updates without polluting the conversation history, and hiding/showing the pane does not disrupt keybindings.

4. **Persist the progress trace for auditability**
   - Thread the normalized updates into the Auditor/session metadata (e.g., last update on error plus an optional `.vizier/sessions/<id>/codex-events.ndjson` file) so operators can review what Codex did after the fact and so future Outcome summaries can cite the last known action when reporting failures (aligns with the Outcome summaries + session logging threads).
   - Document the new behavior briefly in README/AGENTS.md (“Codex runs now expose tool-by-tool progress in the CLI and chat TUI; use `-vv` to see raw events or inspect `.vizier/sessions/<id>/codex-events.ndjson` for audits”).
   - Acceptance: after any Codex run, the session/artifact directory contains the event trace, the Outcome references its path, and docs mention where to look.

## Risks & Unknowns
- Codex’s event schema may evolve; the parser must fail gracefully and the debug dump needs to make schema regressions obvious.
- Streaming too many updates could overwhelm slow terminals; we may need throttling/batching or message deduping so the spinner and TUI stay responsive.
- Persisting detailed tool info might expose sensitive commands or file paths; we need confirmation (or redaction rules) before writing them to disk.
- The chat TUI currently injects updates as assistant messages; removing that side effect might break expectations in parts of the UI that assumed those messages were present.

## Testing & Verification
- Unit tests for the new event-to-update classifier covering representative event payloads (turn started/completed, tool start/completion, command with args, apply_patch/file edit) plus an “unknown event” fallback.
- CLI integration test (with the mock Codex feature or a scripted event source) to assert progress output honors `-q/-v/-vv`, `--progress`, and non-TTY mode, and that the Outcome picks up the last update on failure.
- Ratatui-focused test (or snapshot) that feeds synthetic updates into the `Chat` struct to ensure the progress pane stays bounded, doesn’t corrupt the transcript, and can be toggled.
- Test that session logging (or the new artifact writer) records the event trace and that disabling the feature leaves no files behind.

## Notes
- Touchpoints: `vizier-core/src/codex.rs` (event parsing), `vizier-core/src/display.rs` & `vizier-cli/src/actions.rs` (progress plumbing), `vizier-core/src/chat.rs` (TUI pane), `vizier-core/src/auditor.rs` plus docs.
- Coordinate with the stdout/stderr contract & Outcome-summary threads so the new output fits the unified epilogue strategy.
