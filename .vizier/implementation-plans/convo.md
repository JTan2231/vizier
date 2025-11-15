---
plan: convo
branch: draft/convo
status: draft
created_at: 2025-11-15T16:35:43Z
spec_source: inline
---

## Operator Spec
we need to stop recording conversations on the commit history. conversations no longer have a place in the commit history.

## Implementation Plan
## Overview
Conversations are currently committed into the Git history by `vizier-core/src/auditor.rs::commit_audit`, which creates a `VIZIER CONVERSATION` commit containing the full transcript and exposes its hash to downstream callers/tests (`vizier-cli/src/actions.rs`, `tests/src/lib.rs`). The operator spec requires that conversations no longer appear in commit history, so we need to (1) move the authoritative transcript into the session logging path that already exists in the backlog, and (2) rework every workflow that depends on the old conversation commit so that narrative/code commits remain auditable without exposing the transcript in Git.

## Execution Plan
1. **Finalize repo-local session logging for conversational transcripts**
   - Reuse the active `todo_session_logging_json_store.md` thread: persist every chat/save/clean session under `.vizier/sessions/<session_id>/session.json` with metadata (timestamps, repo state, audited A/M/D/R facts, gate states, model info). Implement atomic write + schema validation as described in the TODO, and surface the file path in the Outcome summary/JSON so operators know where to retrieve the transcript.
   - Extend `Auditor` to treat the session log as the sole source of the conversation text (instead of Git), and ensure the writer runs before we drop conversation commits. Non-TTY/protocol mode must continue to respect the stdout/stderr contract.
   - **Acceptance:** running `vizier save` or `vizier clean` leaves a new `.vizier/sessions/<id>/session.json` that matches the schema, includes every user/assistant message, and is referenced in the CLI epilogue.

2. **Remove conversation commits from `Auditor::commit_audit` while preserving .vizier gating**
   - Update `commit_audit` so it skips the `VIZIER CONVERSATION` commit entirely. We still need to snapshot unstaged code before committing `.vizier` files, so keep the staging safeguards but gate them around the narrative commit only. Delete the `conversation_hash` plumbing or replace it with a lightweight session reference (e.g., session ID/path) that callers can continue to thread through metadata.
   - Adjust `file_tracking::FileTracker::commit_changes`, `CommitMessageBuilder`, and downstream save/clean flows to accept the new reference (or none) so narrative and code commits mention the session ID instead of a Git hash.
   - **Acceptance:** `git log` after `vizier save` shows only the narrative (`VIZIER NARRATIVE CHANGE`) and code commits; no commit includes the transcript. The Outcome still lists the session reference, and staged user work remains untouched.

3. **Refresh CLI UX + docs to reflect the new storage contract**
   - Update `SaveOutcome`, `format_save_outcome`, `clean` summaries, and any other command that prints `conversation=…` so they emit `session=<id or path>` (matching Outcome JSON fields). Ensure `README.md`’s “Commit Workflow” section no longer mentions a conversation commit or the `--no-conversation` flag, and instead explains where transcripts live.
   - Update `CommitMessageBuilder` output so commits reference the session ID/path (still keeping the `Session ID:` header) but never embed the transcript. Cross-link this behavior in `AGENTS.md`/docs so operators know how to audit conversations outside Git.
   - **Acceptance:** CLI epilogues mention session artifacts, docs describe the two-commit workflow, and there are no lingering references to conversation commits or `conversation_hash` in the code.

4. **Revise automated tests + helpers to the new model**
   - Update `tests/src/lib.rs` and `tests/src/main.rs` to stop searching for `VIZIER CONVERSATION` commits. New expectations: `vizier save` creates two commits (narrative + optional code) and produces a session log file. Add assertions that `tests` can read `.vizier/sessions/<id>/session.json` and find the mock Codex response there.
   - Extend any unit tests around `CommitMessageBuilder`/`Auditor` to ensure `conversation_hash` is removed and session references propagate correctly. If the Git plumbing still snapshots staged content, add regression coverage to prove existing staged code survives the save flow.
   - **Acceptance:** integration tests pass without searching for conversation commits, new tests validate session log creation, and CI confirms no code path references the removed concept.

## Risks & Unknowns
- **Sequencing risk:** Removing the conversation commit before session logging is durable would drop the only transcript copy; we must ship the `.vizier/sessions` writer first.
- **Third-party tooling:** Scripts (internal or operator-authored) might parse `conversation=…` from CLI output or rely on three commits per save. We need to communicate the change via README/docs and ensure the new outcome fields remain stable.
- **Historical references:** Older commits will still include transcripts; we should note in docs that the new behavior applies prospectively only to avoid confusion during audits.

## Testing & Verification
- Integration tests under `tests/src/lib.rs` and `tests/src/main.rs` cover: `vizier save` (two commits, session log exists, transcript text stored outside Git), `vizier save` without code changes (still two commits, CLI reports `session=…; code_commit=none`), and `vizier clean` (session log + no conversation commit).
- Unit tests for `CommitMessageBuilder` ensure it never includes the transcript body and that session references render correctly.
- Regression tests confirm `.vizier` commit staging preserves pre-existing staged files and that removing conversation commits does not break push-after-save flows.

## Notes
- Depends on delivering the active `todo_session_logging_json_store.md` thread so transcripts have a new home; coordinate messaging with the Outcome/Session logging updates already underway.
