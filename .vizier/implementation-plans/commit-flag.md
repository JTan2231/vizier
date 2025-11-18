---
plan: commit-flag
branch: draft/commit-flag
status: draft
created_at: 2025-11-18T00:55:58Z
spec_source: inline
---

## Operator Spec
Vizier hard-commits in every flow—plans, reviews, merges, even plain ask runs—so turning automation on implicitly rewrites branch history and leaves no way to inspect Codex’s edits before a commit lands. We need a --no-commit flag that
  keeps those edits staged/dirty instead of auto-committing so operators can review, adjust, or batch them manually. this is a complicated change--please be detailed with our implementation plan

## Implementation Plan
## Overview
Vizier currently auto-commits after every assistant-backed action (ask/save and the draft → approve → review → merge pipeline), which makes it impossible for operators to inspect Codex edits or batch them with their own work before history is rewritten. Adding a `--no-commit` escape hatch lets humans keep those edits staged/dirty inside the relevant worktree, review them, and decide when/how to commit. The change touches both CLI orchestration (`vizier-cli/src/main.rs:1`, `vizier-cli/src/actions.rs:548`, `vizier-cli/src/actions.rs:897`, `vizier-cli/src/actions.rs:1083`) and the git utilities in `vizier-core/src/vcs.rs:1261`, plus documentation of the workflow contract.

## Execution Plan
1. **Define the contract + CLI surface**
   - Add a global `--no-commit` flag (and equivalent config knob) in `vizier-cli/src/main.rs:26` so every subcommand can opt into “apply but do not commit.” Document that it implies “leave worktree dirty, skip push, and surface the worktree path/outcome reminder.”
   - Thread a `CommitMode` enum (`AutoCommit` vs `HoldForReview`) through command dispatch so `run_save`, `inline_command`, `run_draft`, `run_approve`, `run_review`, and `run_merge` can key off one flag rather than duplicating boolean plumbing.
   - Update help text and docs (`README.md:1`, `docs/workflows/draft-approve-merge.md:1`, `AGENTS.md`) so operators know how to resume manually from a held worktree or re-run commands without the flag once satisfied.

2. **Extend Auditor/finalization plumbing used by ask/save**
   - Teach the Auditor flow in `vizier-cli/src/actions.rs:835` and `vizier-cli/src/actions.rs:548` to distinguish “audit ready to commit” from “audit finalized but pending manual commit.” This likely means adding a `finalize(commit_mode)` helper that:
     - For `AutoCommit`, keeps calling `Auditor::commit_audit()`.
     - For `HoldForReview`, stages `.vizier` edits (respecting Pending Commit gates) but skips `git commit`, leaving files dirty in the operator’s checkout while still updating session logs/outcome metadata to show `gate_state=pending (no-commit)`.
   - Ensure downstream push logic is bypassed when no commit was created, and update the Outcome builder to report “Changes staged; run `vizier save` or commit manually” so DAP users understand that history wasn’t rewritten.

3. **Update worktree-based flows (draft/approve/review)**
   - Inside `vizier-cli/src/actions.rs:897` (`run_draft`), add hooks so the generated plan file is written and staged inside the draft branch worktree but `commit_paths_in_repo` is only invoked when `commit_mode == AutoCommit`. When `--no-commit`, keep the temporary worktree around (skip `remove_worktree`) and print its path + branch slug so operators can inspect and manually commit/push later.
   - Apply the same pattern to `run_approve` and `run_review` (`vizier-cli/src/actions.rs:1083` and beyond), ensuring:
     - Code edits land in the draft branch worktree without committing.
     - Review metadata (`.vizier/reviews/<slug>.md`) and plan status updates are staged but not committed.
     - Resume tokens (conflict sentinels) and plan status fields note that the branch is “dirty/pending commit” so merge doesn’t assume history advanced.
   - Guard rails: refuse `--no-commit` when combined with `--yes` auto-apply flows that expect commits, or at least issue warnings that pushes/CI gates will be skipped until a commit exists.

4. **Teach merge helpers to support staged, uncommitted merges**
   - Extend `vizier-core/src/vcs.rs:1261` (and helpers such as `commit_ready_merge`/`commit_in_progress_merge`) so `vizier merge` can perform the non–fast-forward merge and embed the plan in the staged commit message template without actually creating the commit when `--no-commit` is set.
   - Surface the resulting worktree path and remind the operator they are now in a manual `MERGE_HEAD` state; skip CI/CD gate execution until the merge commit is finalized (since there’s no commit to test). Ensure sentinel cleanup logic records that a manual merge is outstanding so rerunning `vizier merge` without the flag can reuse the prepared index.

5. **Outcome/session logging + config updates**
   - Update the Outcome component so every command reports `commit_mode=manual` when `--no-commit` is active, including the worktree path or repository root where dirty changes live. This needs to flow into both the human epilogue and `outcome.v1` JSON once that schema lands, plus `.vizier/sessions/<id>/session.json`.
   - Add a repo-level config default (e.g., `[workflow] no_commit_default = false`) if operators want to prefer manual commits, and ensure `vizier list`/plan inventory call out draft branches holding dirty changes so auditors can spot them.

6. **Documentation + operator guidance**
   - Refresh `README.md:1` and `docs/workflows/draft-approve-merge.md:1` with a subsection that explains how `--no-commit` changes the flow, how to inspect the worktree, and how to resume (manual `git commit`, rerun command without the flag, or use `vizier merge --complete-conflict` once a commit is made).
   - Update AGENTS.md to remind agents that when `--no-commit` is set they must not expect Vizier to push branches or trigger CI gates automatically.

7. **Testing + validation scaffolding**
   - Add integration tests under `tests/src/lib.rs` that run representative commands with `--no-commit` and assert:
     - `git status` in the relevant worktree shows staged/uncommitted changes.
     - No new commits are added to the history.
     - Outcome/session logs record the manual-commit reminder and worktree path.
   - Cover failure recovery (e.g., rerunning `vizier approve` without `--no-commit` once changes have been manually committed) and ensure existing conflict-sentinel tests still pass.

## Risks & Unknowns
- **Worktree lifecycle**: Leaving temporary worktrees around for manual inspection could exhaust disk space or confuse operators; we need a cleanup story (possibly `vizier clean` or explicit instructions) and a sentinel so later runs know a dirty worktree exists.
- **Gate alignment**: CI/CD gates and the Pending Commit gate currently assume commits exist; we must decide whether gates are skipped, marked pending, or re-run once a manual commit lands.
- **Merge ergonomics**: Holding a merge in-progress without a commit leaves Git in MERGE_HEAD state; we must ensure the CLI exits with clear instructions and that future `vizier merge` invocations can resume cleanly.
- **Agent flows**: Codex auto-remediation steps expect to stage and commit interim fixes; we need to confirm `--no-commit` doesn’t break retry logic or leave half-applied patches if the operator aborts.

## Testing & Verification
- Integration tests for `vizier ask --no-commit` and `vizier save --no-commit` asserting no commits were produced, `.vizier` files are staged, and Outcome mentions manual follow-up.
- Draft/approve/review tests verifying that running with `--no-commit` leaves dirty changes inside `.vizier/tmp-worktrees/...` and that rerunning without the flag picks up those staged changes.
- Merge test where `vizier merge <slug> --no-commit` leaves the repo in MERGE_HEAD state with plan metadata staged; confirm that manual `git commit` or a follow-up `vizier merge <slug>` finalizes cleanly.
- Regression tests ensuring `--no-commit` disables automatic pushes and CI/CD gate execution, and that the Observability stack (Outcome JSON + session logs) captures the worktree path and pending status.

## Notes
- Coordinate rollout messaging with the “Commit isolation + gates” thread so auditors know that a lack of new commits may be intentional when `--no-commit` is set.
- Consider adding a `vizier worktrees list/clean` follow-up if operators frequently leave manual-review worktrees behind.
