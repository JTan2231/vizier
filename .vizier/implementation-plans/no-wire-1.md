---
plan: no-wire-1
branch: draft/no-wire-1
status: draft
created_at: 2025-11-20T18:22:44Z
spec_source: inline
---

## Operator Spec
commands that rely on agent changes (approve, merge, etc.) should fail if the configured agent (codex, etc.) fail. there should be no fallback to wire, anywhere.

## Implementation Plan
## Overview
Operators want high-assurance guardrails whenever agents mutate branches or narrative assets, but today the Codex runner silently falls back to the “wire” backend when Codex fails (`vizier-core/src/auditor.rs`, snapshot: Codex backend path falls back to wire). This violates the operator spec (“no fallback to wire anywhere”) and undermines auditability in the Agent workflow orchestration thread. This plan removes all fallback behavior so that `vizier draft/approve/review/merge` (and any other agent-backed command) fail fast with clear Outcome metadata when the selected backend misbehaves, ensuring humans know work was not applied.

## Execution Plan
1. **Revoke fallback configuration surface**
   - Remove `fallback_backend` fields from `AgentSettings`, config builders, and `.vizier/config.toml` parsing (`vizier-core/src/config.rs`), emitting a descriptive error when repos or CLI flags still set the key. Provide a transitional warning message that points to the new policy so operators can clean configs before release.
   - Update repo-owned assets (`example-config.toml`, `AGENTS.md`, README configuration section) to describe the simplified precedence (`backend` only) and clarify that failing agents abort the command. Call out this change in the Snapshot/TODO thread that tracks agent workflow orchestration to keep narratives consistent.

2. **Delete runtime fallback paths in the Auditor**
   - Remove the `fallback_backend` plumbing in `vizier-core/src/auditor.rs` that currently retries Codex prompts via the wire backend when Codex returns an error or times out. Ensure all agent command scopes (`draft`, `approve`, `review`, `merge --auto-resolve-conflicts`, and any `ask/save` flows still using agents) propagate a single backend invocation result.
   - When an agent crashes or reports failure, surface the error (phase, stderr, exit code) through the existing progress/event adapter so operators can diagnose without digging into logs. No secondary agent invocation should occur.
   - Verify the Codex runner (`vizier-core/src/codex.rs`) no longer suppresses exit failures or silently swaps transports; failure should bubble to the calling command so pending worktrees remain untouched.

3. **Tighten CLI Outcomes and user-facing messaging**
   - Ensure `vizier-cli` actions for every agent-backed command treat backend errors as terminal: disposable worktrees stay intact, exit codes reflect failure, and the Outcome line/session JSON explicitly states `agent_backend=codex` (or whichever backend) plus `status=failed`.
   - For workflows that previously relied on fallback for resilience (e.g., `vizier approve` auto-commits, `vizier merge --auto-resolve-conflicts` auto-fix attempts), add clear guidance in their CLI help text and docs that operators must re-run once the backend is healthy rather than expecting wire to pick up the slack.

4. **Expand regression coverage**
   - Add integration tests (under `tests/src/lib.rs`) that simulate Codex failure for representative commands (`vizier draft`, `vizier approve`, `vizier merge --auto-resolve-conflicts`) and assert there is no subsequent wire invocation, the command exits with failure, no commits are created, and the session log references the failed backend.
   - Add config parser tests ensuring `fallback_backend` is rejected with a helpful error, plus documentation tests (or snapshots) verifying README/AGENTS/example-config updates.
   - Update any existing tests or fixtures that referenced fallback behavior (e.g., `tests/test-repo/.vizier/config.toml`) to reflect the new policy.

## Risks & Unknowns
- Removal of fallback may expose latent Codex instability; we need to ensure error messages contain enough context so operators can remediate quickly.
- Some repositories might still ship `.vizier/config.toml` with `fallback_backend`; deciding whether to hard-error vs emit a transitional warning requires coordination with release notes to avoid blocking all commands unexpectedly.
- Commands like `vizier merge` that currently default to wire for non-agent tasks (per `example-config.toml`) need verification so we don’t accidentally drop legitimate wire-only modes; the scope should focus on fallback, not disabling intentionally configured primary backends.

## Testing & Verification
- Integration tests covering Codex failure paths for draft/approve/review/merge should confirm: exit status is non-zero, no `.vizier` or code commits are written, session logs record `agent_backend=codex`, and there is no mention of wire backends in the transcript.
- Unit tests for config parsing fail when `fallback_backend` appears.
- Documentation tests or linting (if available) confirm example configs and AGENTS.md no longer mention fallback semantics.
- Manual smoke test: run `vizier approve <slug>` with an intentionally broken Codex binary path and confirm the CLI aborts immediately with a descriptive Outcome, leaving the draft branch untouched.

## Notes
- Narrative artifacts (snapshot/TODOs) stay unchanged until the implementation lands; this plan only scopes the removal of agent fallbacks.
