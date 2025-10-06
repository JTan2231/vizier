# Goal
Persist each assistant session to the filesystem as structured JSON so it can be reloaded later. Keep Git history as the audit of code changes; JSON is the audit of conversational/runtime context. This enables a future TUI picker and CLI flag to load a prior session.

# Why
- Git history captures code diffs; we also need replayable session metadata (messages, config, decisions, gates crossed).
- Support workflows: resume an interrupted session, compare sessions, branch from a prior point, export/share.

# Scope (MVP)
- Write a single JSON file per session to `.vizier/sessions/<session_id>/session.json` at session end and on key checkpoints (e.g., commit gate accept/reject).
- Immutable once closed; in-flight sessions may write checkpoints.
- No PII beyond what is already in the repo/config. Redact/opt-out capability.

# JSON schema (MVP fields)
- id: stable session id (ulid or uuid-v7).
- created_at, updated_at: ISO-8601 with timezone.
- tool_version: semver/sha of app.
- repo: { root, current_branch, head_sha }
- config_effective: resolved values + provenance per key (CLI|session|profile|default).
- system_prompt_path and hash.
- model: { provider, name, temperature, thinking_level }
- chat: array of messages: { role, content, ts } + optional annotations (e.g., tool_calls, diffs_refs).
- operations: chronological list of actions with minimal provenance:
  - type: one of [proposal, gate_open, gate_accept, gate_reject, apply_diff, revert, test_run, config_change]
  - detail: freeform string or structured payload
  - vcs: { staged: {A:[], M:[], D:[], R:[]}, applied_sha?, restored_sha? }
  - ts: timestamp
- artifacts: references to files written in `.vizier/` (diffs, prompts, logs)
- outcome: { status: open|accepted|rejected|aborted, summary?, commit_sha? }

# UX/Acceptance
- When a session ends, `session.json` exists and validates against the MVP schema (basic JSONSchema).
- Re-running with the same id never overwrites a closed session (write-once); checkpoints use `session.json.tmp` and atomic rename.
- `vizier sessions list` shows: id, created_at, branch, outcome, short summary.
- `vizier --session <id>` loads chat+config, shows header: “Resumed session <id> (branch: X, outcome: Y)” and prohibits edits locked by provenance (e.g., CLI overrides remain immutable).
- Config flag `session_logging: on|off` with default `on`. Redaction: `redact: [secrets, env]`.

# Non-goals (for MVP)
- Full transcript encryption.
- Deduplicating large artifacts.
- Cross-repo sessions.

# Follow-ups
- TUI session picker: searchable list with filters (branch, outcome, date), preview transcript.
- Import/export a session bundle (zip with `session.json` + artifacts).
- Garbage collection policy and `vizier sessions prune`.
- Per-message token stats + cost summaries.

# Notes
- Favor append-only with atomic writes (write tmp, fsync, rename). Keep files <10MB for quick load.
- Add integration tests for write/read + idempotency + provenance locking behavior.

---
Status update:
- Chat path now routes through the Auditor, providing authoritative A/M/D/R facts per chat operation.
- Session persistence is NOT wired yet; sessions are not being saved.

Next steps to close gap:
- Hook session writer at chat operation boundaries to persist audited facts + workflow metadata (workflow_type=chat, thinking_level, mode, timestamps, repo state, outcome summary).
- Ensure atomic write to session.json (temp file + rename) and schema validation.
- Expose session path/location in the Outcome epilogue for discoverability.

Acceptance criteria additions:
- After any chat operation, a session JSON artifact exists at the configured log location and validates against the schema.
- The artifact includes Auditor facts (A/M/D/R, changed paths), gate state, and Outcome identifiers.
- With --mode protocol, emit the session record path as part of the final JSON Outcome.


---

Persist sessions as JSON artifacts and surface path in Outcome.
Describe behavior:
- After each chat/operation, write a structured session record to .vizier/sessions/<session_id>/session.json, updating checkpoints at key transitions (e.g., gate open/accept/reject) and finalizing at session end. The Outcome epilogue (human and outcome.v1 JSON) includes the session file path for discoverability. Honors session_logging on/off and redaction settings; no interactive prompts are introduced. (thread: session-logging; snapshot: Running Snapshot — updated)

Acceptance Criteria:
- File creation: A session.json exists under .vizier/sessions/<id>/ after any chat operation and at session end; includes workflow_type (chat), mode, thinking_level, repo state, and Auditor/VCS facts (A/M/D/R counts and changed paths), gate state, and a concise outcome summary/identifiers.
- Checkpoints + immutability: In-flight writes use a temporary file and atomic rename; once a session is closed, subsequent runs never overwrite it. Checkpoint updates are reflected in updated_at.
- Outcome linkage: CLI epilogue prints the session file path; outcome.v1 JSON includes session.path. In protocol mode, stdout carries only JSON/NDJSON and includes the same path; no ANSI ever.
- Config levers: session_logging default on; can be disabled via config/flag. Redaction list (e.g., secrets, env) applied before writes. CLI flags override config.
- Safety bounds: Closed-stdin never blocks session persistence. Non-TTY contexts behave identically (no ANSI). Files remain reasonably small for quick load.
- Tests: Integration tests validate (a) file creation and schema validity, (b) atomic write behavior and idempotency, (c) redaction applied, (d) disable path respected, (e) Outcome/JSON includes session.path across chat and protocol modes.

Pointers:
- vizier-core/src/chat.rs (chat boundaries/hooks), vizier-core/src/auditor.rs (facts), vizier-core/src/display.rs and vizier-cli/src/actions.rs (Outcome epilogue), config schema for session_logging/redact.

Implementation Notes (safety/correctness):
- Use write-to-temp + fsync + atomic rename for each checkpoint/finalization; never partially written JSON. Validate against a minimal MVP schema before rename.