Advancement: define outcome.v1 schema and CLI epilogue mapping

Outcome JSON schema v1 (safety/correctness requires specificity)
- Emit exactly one final object per operation (or as the last NDJSON event in protocol mode).

{
  "schema": "outcome.v1",
  "action": "chat|save|agent_run|agent_abort|init|snapshot_update",
  "mode": "chat|protocol",
  "success": true|false,
  "elapsed_ms": <number>,
  "auditor": {
    "files": {"added": <n>, "modified": <n>, "deleted": <n>, "renamed": <n>},
    "lines": {"added": <n>, "removed": <n>}
  },
  "pending_commit": {
    "state": "none|open|accepted|rejected|skipped",
    "reason": "auto_commit|non_interactive|no_changes|confirmation_required|n/a"
  },
  "commits": {
    "conversation_sha": "<sha>"|null,
    "vizier_sha": "<sha>"|null,
    "code_sha": "<sha>"|null
  },
  "agent": {
    "todo": "<id-or-name>"|null,
    "branch": "<branch-name>"|null,
    "commit_count": <n>|null,
    "pr_url": "<url>"|null
  },
  "gc_todos": {
    "deleted": [{"name": "<file>", "reason": "duplicate_of|superseded_by|empty|orphaned", "ref": "<canonical>"?}],
    "skipped": [{"name": "<file>", "reason": "protected|active_agent_branch"}]
  },
  "session_log_path": "<path>"|null,
  "warnings": ["<string>", ...],
  "errors": ["<string>", ...]
}

CLI epilogue (human) mapping
- Header: "Outcome: <action> — <success|failed> in <elapsed>"
- Changes: "Files A/M/D/R: a/m/d/r; Lines +/-: +x/-y"
- Commits: summarize present SHAs by category.
- Gate: "Pending Commit: <state> (<reason>)" when applicable.
- Agent (when present): "Agent: TODO <id>, branch <name>, commits <n>, PR <url|local review>"
- GC (when present): "GC: deleted <n>, skipped <n>" with short bullet list up to N=5.
- Session: path when present.

Tests additions
- Validate schema fields presence/types for chat/save/agent_run cases.
- Ensure human epilogue mirrors JSON facts exactly (spot-check key fields) and suppresses in --quiet.

Protocol mode alignment
- In protocol mode, only NDJSON events are printed on stdout, culminating in the outcome.v1 object above; no human epilogue. Deterministic event ordering enforced: status* → outcome.

Cross-links
- Stdout/stderr contract TODO: governs stream/verbosity behavior.
- Mode split TODO: defines protocol constraints and exit codes.


---

