Context:
- Users lack visibility into errors and what actions the app is taking. Current TUI has ad-hoc status spinner (tui/src/lib.rs display_status) and drops detailed causes; chat TUI swallows errors in async task and prints only via println! on exit; file list TUI loses context when shelling to editor.

Deliverables (tight, code-anchored):

1) Replace opaque error swallow in chat.rs send_message join path with surfaced Assistant error frame
- File: tui/src/chat.rs
- Change: When wire::prompt_with_tools_and_status returns Err, capture error string and push a wire::types::Message { message_type: Assistant, content: format!("Error: {e}\nSee logs for details"), input/output_tokens=0 } into app.messages instead of unwrap().
- Also set receiving_handle back to None to stop spinner.
- Rationale: Today we .unwrap() in the spawned task, which panic-aborts the join. Showing a concrete Assistant message keeps the narrative visible in-session.

2) Stream structured progress/events into chat window via rx with prefixes and levels
- File: tui/src/chat.rs
- Change: Treat tx/rx channel as a structured status bus. Define message prefixes: "info:", "warn:", "error:", "tool:". In run_chat loop, on rx.try_recv(), map prefix to Color (info=White, warn=Yellow, error=Red, tool=Cyan) and render as Assistant messages with styled first token (retain existing message log type).
- Additionally, increment a per-level counter in Chat for quick summary in chat title (e.g., W:x E:y) alongside token counts.

3) Add an on-screen error panel in list_tui for actionable failures
- File: tui/src/lib.rs
- Change: Extend App with Vec<String> errors and last_action: String. Wrap fallible operations (fs::read_dir, read_to_string, enter_directory editor spawn) and push concise errors (including path) into errors.
- UI: In ui(), split vertical layout into [files | preview] and add a bottom row Constraint::Length(3) with a Paragraph titled "Events" that shows the last 3 entries of App.errors joined by newlines, color-coded red for lines starting with "Error:" and yellow for "Warn:".
- Behavior: When opening editor, if command fails, capture stderr/exit and append Error with editor and path; do not std::process::exit(0) — instead return to TUI and refresh.

4) Unify spinner/status with cancellable task scopes and cleanup
- File: tui/src/lib.rs display_status and call_with_status
- Change: Introduce Status::Info(String), Status::Warn(String) variants and colorize accordingly. Ensure display_status clears the spinner line on Done by printing "\r  \r" then a final "Done" or last message without spinner. Remove comment about messy carriage returns by implementing proper line clearing using crossterm::terminal::Clear(ClearType::CurrentLine).
- In call_with_status, propagate original API model and token counts instead of dummy GPT5 by requiring the closure to return a full Message or an error; on Err, also send Status::Error and return a synthetic Assistant message with content prefixed "Error:" only if caller requests; otherwise bubble error up to caller.

5) Add verbose logging toggle with VIZIER_DEBUG env var
- Files: tui/src/chat.rs and tui/src/lib.rs
- Change: Read std::env::var("VIZIER_DEBUG").is_ok(). If true, append a hidden System message at session start: "[debug enabled]" and also write errors to stderr with timestamps. In chat.rs title bar, append "DBG" when enabled.

6) Wire tool-level telemetry from prompts crate into TUI status bus
- Files: prompts/src/tools.rs and prompts/src/lib.rs
- Change: For each tool invocation, send progress over provided Sender<String> with structured prefixes (tool:start <name>, tool:output <summary>, tool:done <name>, warn:<msg>, error:<msg>). Ensure chat.rs passes tx into prompt_with_tools_and_status and that tools use it consistently.

7) Regression harness: panic-safety and UI resilience
- File: tui/src/chat.rs tests (new) and tui/src/lib.rs tests (new)
- Add async tests ensuring: a) spawned task error does not crash TUI; b) rx message with "error:" prefix renders into messages; c) display_status clears spinner. Use a mock wire::prompt_with_tools_and_status that returns Err and one that streams messages then Ok.

Notes:
- Keep narrative continuity: errors and events appear inline where users already look (chat log and bottom Events pane).
- Avoid generic "investigate" tasks; implement concrete rendering and propagation points.
Tighten integration with .vizier logs and SAFE_APPLY gate to align with Snapshot Thread C:

8) Emit .vizier/logs/errors.jsonl records from TUI-visible errors
- Files: tui/src/lib.rs and tui/src/chat.rs
- On every pushed App.errors entry and every Assistant "Error:" message, append a JSON line to .vizier/logs/errors.jsonl with fields: ts (rfc3339), source (tui|chat), action (read_dir|read_file|editor_spawn|llm_request|tool_call), path (opt), message, stderr (opt), correlation_id (opt). Ensure parent dir exists.

9) Correlate TUI events with prompts tool audit trail
- Files: prompts/src/lib.rs
- When emitting plan and audit events, include correlation_id from caller if provided. Update call sites in chat.rs to generate a UUID per user message, pass it to prompt_with_tools_and_status, and include it in TUI error/event entries.

10) SAFE_APPLY-aware UI affordance
- Files: tui/src/chat.rs
- If SAFE_APPLY is not set, display a non-intrusive yellow banner message in chat: "Dry-run: tools not executed. Use SAFE_APPLY=1 to apply." Ensure it appears once per session and again after each user message that triggers a plan-only response.

Acceptance additions:
- Every surfaced error also exists in errors.jsonl with correlation_id when applicable.
- Dry-run state is clearly communicated in the chat UI and correlates with plan.jsonl entries.

---

Integrated with TUI status bus and errors.jsonl. Chat error surfaces as Assistant frame, structured prefixes map to colors, bottom Events pane shows last entries, spinner cleanup with proper Clear. Add VIZIER_DEBUG toggle and regression tests. Correlate with audit via correlation_id passed from chat into prompts and tools.

---

Refinement (2025-09-08) — Consolidate with Snapshot Thread C and wire JSONL logs across TUI and core tools.

- Error reporting
  • Files: vizier-core/src/auditor.rs, vizier-tui/src/chat.rs, vizier-tui/src/lib.rs
  • Action: Provide append_error(event) that writes .vizier/logs/errors.jsonl lines with {ts, source, action, path?, command?, stderr?, message}. TUI calls on editor and tool failures; core tools call on process errors. Ensure directory creation on first run.

- Conversation/LLM audit
  • Files: vizier-core/src/auditor.rs
  • Action: add append_llm_audit(event) writing to .vizier/logs/llm_audit.jsonl with {ts, thread_id?, correlation_id, tool, args_preview, result_preview, token_in, token_out, duration_ms}.

- User event tracing
  • Files: vizier-tui/src/chat.rs, vizier-tui/src/lib.rs
  • Action: emit user_action events to .vizier/logs/events.jsonl for key actions (open_editor, commit_save, navigate_dir) with {ts, source:"tui", action, target?}.

- SAFE_APPLY gate integration
  • Files: vizier-core/src/auditor.rs
  • Action: When SAFE_APPLY is unset/false, record plan entries to .vizier/logs/plan.jsonl and skip side-effects; when true, execute and record outcomes/errors.

Acceptance: Errors are logged with actionable fields; LLM/tool interactions produce audit lines; user actions are traceable; SAFE_APPLY toggles dry-run behavior with plans recorded.

---

