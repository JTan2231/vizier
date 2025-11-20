---
plan: squashing
branch: draft/squashing
status: draft
created_at: 2025-11-20T02:19:25Z
spec_source: inline
---

## Operator Spec
we need a flag (something like --squash) that determines whether vizier merge squashes commits before merging. We want vizier merge to land exactly two commits on the target: one Vizier code commit that already includes deleting the plan doc, refreshing snapshot/TODOs, and any CI/CD auto-fix changes, followed by the merge commit that embeds the
  plan text. Today the flow always adds a standalone refresh/removal commit on the draft branch before merging, then writes the merge commit, and CI/CD auto-fix (if enabled) can add further fix commits on the target—so you end up with
  multiple Vizier-authored commits rather than a single code commit plus the merge. we want this to be the default behavior, and configurable just like any other flag

## Implementation Plan
## Overview
`vizier merge` currently lands every commit from `draft/<slug>` plus an additional plan-refresh commit, so target branches pick up several Vizier-authored commits before the merge commit. That conflicts with the active “Git hygiene + commit practices” and “Agent workflow orchestration” threads in `.vizier/.snapshot`, which ask for predictable, auditable history on the primary branch. This work introduces a squash-first merge path (default) that condenses all implementation edits—plan doc removal, snapshot/TODO refresh, and any CI/CD auto-fix output—into a single code commit before emitting the merge commit that embeds the approved plan. Operators keep the ability to opt out via flag/config, but the default experience now produces the “exactly two commits per plan” story the spec demands.

## Execution Plan
1. **Expose the squash knob across config, CLI, and docs**
   - Extend `MergeConfig` in `vizier-core/src/config.rs` with a `squash_default: bool` (default `true`), parse `[merge] squash = true|false` (and legacy aliases if needed), and surface it via `config::get_config().merge`.
   - Update `example-config.toml` and `.vizier/config.toml` comments to document `[merge] squash = true` plus how CLI flags override repo defaults, mirroring the existing CI/CD gate story.
   - Add `--squash`/`--no-squash` switches to `MergeCmd` (`vizier-cli/src/main.rs`) with clear help text (“squash implementation commits before creating the merge commit”). Make them mutually exclusive and default to the config value when neither is set.
   - Thread the resolved boolean through `resolve_merge_options` into a new `squash: bool` field on `MergeOptions`, so downstream logic can branch without rereading config.
   - Update operator-facing docs (`README.md`, `docs/workflows/draft-approve-merge.md`, AGENTS.md) to state that `vizier merge` now squashes by default, how to disable it, and what audit trail to expect (implementation commit + merge commit).

2. **Create an explicit “implementation commit” pipeline for squashed merges**
   - After the existing plan-branch cleanup (`refresh_plan_branch`), split `run_merge` into two pathways:
     - Legacy path (`--no-squash`) keeps today’s `prepare_merge → commit_ready_merge` flow.
     - Squash path computes the merge diff via `vcs::prepare_merge`, but instead of committing a two-parent merge immediately, it uses a new helper (e.g., `vcs::commit_squashed_merge`) to create a single-parent commit from the prepared tree. This commit should carry a descriptive message such as `feat: apply plan <slug>` plus context (plan summary, optional operator note) so history stays meaningful.
   - Update `refresh_plan_branch` to return the `source_oid` (plan branch tip) needed later for branch deletion, even though target commits no longer inherit source parents directly.
   - Ensure the working tree/index reflect the squashed tree before committing so CI/CD steps and manual inspection operate on the exact bits destined for the code commit.

3. **Re-sequence the CI/CD gate and auto-fix logic around the new workflow**
   - Teach `run_cicd_gate_for_merge` to run before the merge commit when `squash == true`. The gate should examine the repository after the implementation commit has been created (or after the squash diff has been applied but before committing, if we delay the commit until the gate passes).
   - When auto-remediation is enabled and the gate fails in squash mode, run Codex fixes in-place and amend the pending implementation commit instead of creating extra fix commits. Capture amend counts and summaries so `CicdGateOutcome` can still report what happened (e.g., `fixes=[amended implementation commit (attempt 2)]`).
   - Retain the existing “commit per fix” behavior for non-squash merges to avoid regressions.
   - Adjust `finalize_merge` output to mention whether fixes were folded into the implementation commit or emitted as standalone commits, preserving the auditability requirement from the Agent workflow thread.

4. **Emit the final merge commit after the implementation commit and gate succeed**
   - Once the squashed commit exists and the CI/CD gate reports success, recompute a merge (it should now be a no-op) and create the standard merge commit that embeds the plan document. Reuse the existing `build_merge_commit_message` for body formatting but make sure the merge parent list uses the implementation commit (HEAD) and the original `draft/<slug>` tip so branch deletion safety checks still work.
   - Ensure `vizier merge --note` applies to the merge commit only, not the implementation commit, matching prior behavior.
   - Preserve the `token_usage` and session logging plumbing so both commits reference the same Auditor session details.

5. **Update conflict handling and resumption semantics for squash merges**
   - Extend `MergeConflictState` to record whether the merge began in squash mode and to capture both the planned implementation-commit message and the final merge message. `try_complete_pending_merge` should branch accordingly:
     - For non-squash, continue committing the two-parent merge once conflicts are cleared.
     - For squash, finalize the MERGE index into the implementation commit (single-parent), clean up Git’s merge state, then resume the normal squash pipeline (CI/CD gate + final merge commit). Return enough metadata (e.g., implementation commit SHA, source tip SHA) so callers can proceed without re-merging.
   - Propagate the squash flag into `handle_merge_conflict` and Codex auto-resolution so successful auto-resolve runs produce the implementation commit rather than immediately finalizing the merge.
   - Update the `--complete-conflict` UX copy in `run_merge` and `docs/workflows/draft-approve-merge.md` to explain the two-phase completion (“first we finalize the implementation commit, then Vizier finishes the metadata merge”).

6. **Testing & instrumentation**
   - Expand the integration suite in `tests/src/lib.rs`:
     - New test proving default `vizier merge` results in exactly two target-side commits (inspect `git log` and assert the first includes plan doc deletion, the second is the merge embedding the plan).
     - Coverage for `vizier merge --no-squash` to confirm legacy behavior is untouched.
     - CI/CD gate scenarios (pass/fail/auto-fix) that assert squash mode never creates extra commits and that amend counts appear in the Outcome line.
     - Conflict-resolution regression tests: one for manual `--complete-conflict` while squashing, another for `--auto-resolve-conflicts --squash`.
   - Add unit tests around the new config parsing (`merge.squash`), the new commit helper, and any amended data structures (`CicdGateOutcome`, conflict state).

7. **Documentation & operator guidance**
   - Refresh README, AGENTS.md, and the workflow doc to describe:
     - The default squashed history contract and the new flags/config keys.
     - How CI/CD auto-fixes now amend the implementation commit instead of producing extra commits, including any implications for auditing (e.g., commit trailers showing fix attempts).
     - Conflict recovery steps in squash mode.
   - Mention the change in `.vizier/.snapshot` once code lands to tie back to the Git hygiene thread (not part of this plan, but call out that the snapshot will need an update alongside implementation).

## Risks & Unknowns
- **Conflict resumption complexity**: teaching the existing sentinel/`--complete-conflict` flow to produce a single-parent implementation commit (and then continue into the merge/Gate path) touches sensitive state machine code; we need careful sequencing to avoid leaving repos stuck in `MERGE` state.
- **CI/CD auto-fix amends**: reusing `IntentionalCommitSession` for commit amendments (or building a new amend helper) risks disturbing staged work or losing metadata if we don’t carry over trailers/session IDs. We need a clear story for how amended commits retain Auditor notes.
- **Tree application mechanics**: applying the prepared merge tree to the working tree/index without immediately committing requires precise libgit2 usage; mistakes could drop untracked files or bypass our intentional-staging rules.
- **Performance/regression**: recomputing the merge twice (once for the implementation commit, once for the metadata-only merge) may expose new edge cases (e.g., fast-forward detection, branch deletion safety). Need to ensure the second merge path gracefully handles “already up to date” situations.

## Testing & Verification
- Integration tests that:
  - Run `vizier merge <slug>` on the fixture repo and assert `git rev-list --count <old..new>` equals 2 and the first commit removes the plan doc.
  - Exercise `--no-squash` to ensure the old multi-commit timeline still exists when requested.
  - Simulate CI/CD gate failures with auto-fix enabled and confirm `git log` still shows a single implementation commit (verify amend count via `git show` trailers).
  - Trigger merge conflicts (manual and Codex auto-resolve) under squash mode and ensure the resolution path completes, yielding two commits.
- Unit tests for config parsing (`merge.squash`), new helper methods in `vizier-core::vcs`, and any new data structures (e.g., conflict-state serialization).
- Manual or scripted verification that docs/examples reference the new flag and default, plus a `vizier merge --help` snapshot to confirm the CLI text.

## Notes
- Snapshot/TODO updates will be required post-implementation to reflect the new merge contract (ties back to Git hygiene thread).
