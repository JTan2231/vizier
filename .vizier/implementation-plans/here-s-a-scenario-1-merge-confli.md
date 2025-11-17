---
plan: here-s-a-scenario-1-merge-confli
branch: draft/here-s-a-scenario-1-merge-confli
status: review-ready
created_at: 2025-11-16T19:37:45Z
spec_source: inline
implemented_at: 2025-11-17T01:46:30Z
reviewed_at: 2025-11-17T01:51:20Z
---

## Operator Spec
here's a scenario: 1. merge conflict after vizier merge 2. i fix it manually 3. i have to run vizier save && git commit -m "merge conflict" in order for vizier to capture the changes + for the merge conflict to be resolved. i shouldn't have to have that extra git commit on there. we should add a flag to vizier merge specifically for this scenario--if we're not currently in a resolved merge conflict, it should fail

## Implementation Plan
## Overview
Operators currently hit a merge conflict during `vizier merge`, resolve the files manually, and then have to run `vizier save` plus an ad-hoc `git commit` just to conclude the merge. That extra commit violates the documented draft→approve→merge choreography in `docs/workflows/draft-approve-merge.md` and leaves history cluttered with temporary conflict messages. We need a dedicated `vizier merge` flag that finalizes only when a previous Vizier-led merge is in the “conflicts resolved, ready to commit” state and otherwise fails fast, so conflict resolution stays auditable without additional commits.

## Execution Plan
1. **Extend the CLI surface with a conflict-completion flag**  
   - Add a boolean like `--complete-conflict` (name can still be tuned, but should read as “only finish a prior merge”) to `MergeCmd` in `vizier-cli/src/main.rs` and thread it through `MergeOptions` in `vizier-cli/src/actions.rs`.  
   - Flag must be mutually orthogonal to existing knobs (`--auto-resolve-conflicts`, `--keep-branch`, etc.) and should appear in `vizier merge --help` plus README/docs summaries.  
   - Acceptance: running `vizier merge --help` shows the new flag with wording that makes it clear it only finalizes a previously conflicted merge.

2. **Gate merge completion logic on the new flag**  
   - Teach `run_merge` to branch early when `opts.complete_conflict` is set: call `try_complete_pending_merge` and fail if it returns `None` (no sentinel), if the repo is not in `RepositoryState::Merge`, if conflicts remain, or if the current branch differs from the stored target.  
   - When the helper succeeds, immediately call `finalize_merge` with the returned OIDs so the merge commit preserves the stored plan metadata path.  
   - When the flag is **not** set, the existing behavior stays: `try_complete_pending_merge` may still auto-complete if metadata is present; otherwise we proceed with a fresh merge attempt.  
   - Acceptance: `vizier merge <slug> --complete-conflict` succeeds only when `.vizier/tmp/merge-conflicts/<slug>.json` exists, the repo is mid-merge with no conflicts, and files are staged; in every other state it emits a descriptive error and exits non-zero without touching branches.

3. **Improve conflict-state feedback and logging**  
   - Consider returning a richer enum from `try_complete_pending_merge` (e.g., `PendingMerge::Ready`, `PendingMerge::HasConflicts`, `PendingMerge::MissingState`) so we can explain why `--complete-conflict` failed (“no stored merge metadata”, “still-conflicted paths: …”, etc.) while keeping existing resumptions intact.  
   - Ensure the merge sentinel file is cleared only when the merge actually finalizes so repeated `--complete-conflict` invocations remain idempotent.  
   - Update any user-facing `display::info/warn` strings so they explicitly direct users to the new flag when conflicts are detected or when they try to finish without first resolving them.  
   - Acceptance: after resolving conflicts and staging files, re-running `vizier merge <slug> --complete-conflict` removes the sentinel and prints the same merge summary as a non-conflict path; invoking the flag earlier reports which prerequisite is missing without deleting the sentinel.

4. **Document the workflow change**  
   - README Core Commands section and `docs/workflows/draft-approve-merge.md` need a subsection that explains how to resolve conflicts: “Resolve locally, stage, then run `vizier merge <slug> --complete-conflict` (fails if no pending merge).”  
   - If AGENTS.md or other operator references mention conflict handling, align them with the new flag for clarity.  
   - Acceptance: published docs clearly describe when to use the flag, emphasize that it fails when no pending merge exists, and link to conflict sentinel behavior so multi-agent runs are consistent.

5. **Cover the new behavior with tests**  
   - Extend the integration suite in `tests/src/main.rs` (and the `IntegrationRepo` tests in `tests/src/lib.rs` if they assert merge flows) with two cases: (a) simulate a failed merge, resolve/stage files, and confirm `vizier merge <slug> --complete-conflict --yes` produces the merge commit without extra commits; (b) invoke the flag when no pending merge exists and assert it exits non-zero with the expected error.  
   - Re-run existing merge conflict tests to ensure the default rerun path is still supported without the flag.  
   - Acceptance: new tests fail before the change, pass afterward, and document the failure mode message so future regressions are caught.

## Risks & Unknowns
- Need to choose a flag name that is both concise and self-explanatory; we should confirm with maintainers before hard-coding it if there’s an established naming style.  
- `try_complete_pending_merge` currently returns `Ok(None)` when metadata is missing; we must be careful not to treat that as success under the new flag, and we need to ensure we don’t delete partial conflict data prematurely.  
- Operators may already rely on the auto-resume behavior without a flag; we must double-check that our new branch does not regress that path and that help/docs make the distinction between implicit resume (rerun command) and explicit “only resume” mode clear.

## Testing & Verification
- Integration tests in `tests/src/main.rs`/`tests/src/lib.rs` covering: 
  - Successful completion path with `--complete-conflict`. 
  - Failure when the flag is used with no merge in progress. 
  - Regression tests from `test_merge_conflict_manual_resume` still pass without the flag.  
- Manual smoke: simulate the operator sequence (run merge → resolve conflicts → `vizier merge --complete-conflict`) in a local repo and ensure no extra commits are created.  
- `cargo fmt`/`cargo clippy` to ensure new CLI flag/help text builds cleanly.

## Notes
- No architecture-doc gate changes are required here, but update snapshot/TODO threads if conflict handling policy shifts.  
- Coordinate messaging with the session logging/outcome work so the CLI epilogue highlights when the conflict-completion path fires (even if that’s a future enhancement).  
- Narrative delta this run: captured the new conflict-completion flag plan so reviewers can sign off before implementation.
