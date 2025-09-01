Context:
- We need a centralized, auditable record of all LLM activity (inputs, tool calls, outputs, token usage), tied to existing conversation management. Current TUI chat swallows some errors and only partially surfaces events; prompts/tools emit progress ad-hoc. There is no durable, queryable log across sessions.

Deliverables (code-anchored, cohesive):

1) Introduce ConversationEvent and AuditRecord types and persist them per session
- Files: prompts/src/lib.rs, prompts/src/tools.rs, tui/src/chat.rs, cli/src/main.rs
- Define in prompts/src/lib.rs:
  - pub enum ConversationEvent { UserMessage{content, ts}, AssistantMessage{content, ts, model, input_tokens, output_tokens}, ToolStart{name, args_json, ts}, ToolOutput{name, summary, ts}, ToolDone{name, ts, ok}, Error{scope, message, ts} }
  - pub struct AuditRecord { session_id: Uuid, event: ConversationEvent }
- Add a trait AuditSink { fn write(&self, rec: &AuditRecord) -> anyhow::Result<()>; }
- Provide a default FileAuditSink that appends line-delimited JSON to $VIZIER_AUDIT_DIR/<session_id>.jsonl (create dir if missing). Use serde_json::to_writer.
- Expose a lightweight Audit handle (cloneable) that wraps Arc<dyn AuditSink + Send + Sync> and has convenience fns audit.user(...), audit.assistant(...), audit.tool_start(...), etc.

2) Thread Audit through conversation lifecycle and tools
- Files: prompts/src/lib.rs, prompts/src/tools.rs, tui/src/chat.rs
- Extend prompt_with_tools_and_status signature to accept audit: Audit. On each major stage:
  - When receiving user input in chat.rs, immediately audit.user(content).
  - Before sending to LLM, audit.assistant("request", model, est_input_tokens=calc, output_tokens=0) with a marker field phase: "request".
  - After LLM response chunks are assembled, audit.assistant(final_content, model, input_tokens, output_tokens).
  - For every tool call in tools.rs, emit tool_start/tool_output/tool_done and error if failed.
  - On any error path currently surfaced to TUI, also emit audit.error(scope="chat"/"tool"/"io", message).

3) Stable session identity and rotation policy
- Files: tui/src/chat.rs, cli/src/main.rs, prompts/src/lib.rs
- Generate a session_id=Uuid::now_v7() at conversation start and show it in the chat title bar (shortened 8 chars). Pass to Audit so all records share the same id. When chat restarts, a new session id is created. Add env VIZIER_AUDIT_ROTATE=megabytes (default 50). FileAuditSink should rotate current JSONL when size exceeds limit by renaming to <session_id>.<n>.jsonl and starting a new file.

4) Redaction hooks for sensitive inputs
- Files: prompts/src/lib.rs, tui/src/chat.rs
- Add AuditRedactor trait { fn redact(&self, event: &ConversationEvent) -> ConversationEvent }. FileAuditSink should apply an Optional redactor before writing. Provide a DefaultRedactor that masks secrets detected by common variable names (api_key, token, password) and strips file contents over 50KB, replacing content with "<omitted:large>".
- Chat UI: if VIZIER_AUDIT_REDACT=0 set, skip redaction; default is on.

5) CLI command to view audit tail and filter
- Files: cli/src/main.rs, cli/src/config.rs, README.md
- Add subcommand: vizier audit tail [--session <id-prefix>] [--level <user|assistant|tool|error>] [--since <duration like 10m>] [--follow]. Implement by reading JSONL files in $VIZIER_AUDIT_DIR, filtering by session_id and event variant, pretty-printing concise lines (ts, level, 80-col truncated content). Support --follow using notify crate + file tailing.
- Document env var VIZIER_AUDIT_DIR (default: ~/.local/state/vizier/audit on Linux, ~/Library/Application Support/vizier/audit on macOS; use dirs crate).

6) TUI inline audit inspector pane
- Files: tui/src/chat.rs
- Add a toggle (key: a) to open a right-side pane "Audit" showing the last N events from the current session (read directly from the JSONL to ensure parity with persisted data). Render colored by level: user=Green, assistant=Cyan, tool=Magenta, error=Red. Provide a quick filter (keys 1-4) to toggle levels.

7) Tests: persistence, rotation, redaction
- Files: prompts/src/lib.rs (tests), cli/src/main.rs (tests), tui/src/chat.rs (tests)
- Cover: a) Audit writes JSONL with valid serde shapes for all variants; b) Rotation triggers at size threshold and new file is used; c) Redactor masks api_key fields and elides large blobs; d) TUI inspector correctly tails and renders events.

Notes:
- Keep codepaths minimal: avoid scattering logging; centralize through Audit handle.
- All logging must be non-blocking: use a bounded crossbeam channel in FileAuditSink and a background writer thread; drop with warn when buffer full to avoid UI stalls.
- This aligns with existing error/status bus and will replace ad-hoc printlns.