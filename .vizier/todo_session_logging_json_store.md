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

