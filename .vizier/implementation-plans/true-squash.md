---
plan: true-squash
branch: draft/true-squash
status: draft
created_at: 2025-11-21T04:39:37Z
spec_source: inline
---

## Operator Spec
Current: Vizier’s “squash” builds one synthetic implementation commit from the target-vs-plan tree diff (merge-tree style), ignoring the plan branch’s internal commit graph.
  Effect: The target ends up with exactly one implementation commit plus the merge commit; the original plan commits are only reachable via the merge parent, not preserved on the target.
  Goal: Rework squash to rebase/cherry-pick the plan range onto the target, then do a soft-reset-style squash over that full range (git reset --soft <merge-base>/HEAD~N → single commit) before writing the merge commit.

## Implementation Plan
## Overview
Rework the default squashed merge path so Vizier reapplies the plan branch commits onto the target before squashing. Instead of a merge-tree synthetic diff, the implementation commit will come from cherry-picking/rebasing the plan range, then soft-resetting and committing once, keeping the “implementation commit + merge commit” shape while honoring the plan’s actual history. This affects operators running `vizier merge` with the default squash behavior; `--no-squash` stays legacy.

## Execution Plan
1) Define the new squash algorithm and data flow  
- Identify where the current squash path is built (`vizier-cli/src/actions.rs` uses `prepare_merge` + `commit_squashed_merge` and merge-ready helpers in `vizier-core/src/vcs.rs`).  
- Specify the inputs needed for the new path: merge base between target and plan, ordered plan commit range, starting target HEAD (future parent), plan tip for the merge parent, and a resume token for conflicts.

2) Add VCS helpers for plan-range reapply and soft-squash  
- Implement a helper in `vizier-core/src/vcs.rs` to compute the merge base and cherry-pick/rebase the plan range onto the current target HEAD (3-way apply with index writes). Return structured outcomes: success with applied count/tip, or conflict with enough context to resume.  
- Introduce a soft-squash helper that, given the starting HEAD and applied range length, performs a `--soft` reset back to the starting commit, stages the combined changes, and writes the single-parent implementation commit (allow empty iff current behavior allowed it).  
- Ensure helper leaves the repo in a clean state or a clear git state (CherryPick/Rebase) that downstream conflict handling can recognize.

3) Rewire the squashed merge workflow in `vizier merge`  
- Replace the merge-tree-based squash path with: compute plan range → apply commits onto target → soft-squash commit with the existing implementation message → run the CI/CD gate → write the merge commit with parents `[implementation, plan-tip]`.  
- Preserve the legacy path for `--no-squash` untouched. Keep CI/CD gate auto-fix behavior amending the implementation commit when `auto_resolve=true`.  
- Ensure branch cleanup, plan document removal, and session/outcome metadata remain intact.

4) Conflict handling and resume support  
- Extend `--auto-resolve-conflicts`/`--complete-conflict` handling to the cherry-pick/rebase flow: store a sentinel with branch names, starting HEAD, plan tip, merge base, and queued commits so manual or agent resolution can finalize the implementation commit consistently.  
- Update user-facing conflict messages to reflect the new flow (resolve conflicts from the cherry-pick/rebase, then rerun `vizier merge <slug> --complete-conflict`), and ensure resume logic knows whether to finalize the soft-squash or merge commit.  
- Keep safety rails: reject resume if the repo state/branch does not match the sentinel.

5) Documentation updates  
- Refresh README and `docs/workflows/draft-approve-merge.md` to describe the new default squash behavior (plan-range cherry-pick → soft reset → single implementation commit) and clarify that `--no-squash` keeps the legacy merge-tree parentage.  
- Call out how conflicts/resume behave under the new path and that CI/CD gate still runs against the squashed implementation commit.

6) Testing & verification harness  
- Update existing merge squash/no-squash integration tests to pass under the new flow (parentage expectations unchanged).  
- Add coverage for: (a) plan branch with multiple commits applied then squashed (tree matches plan tip, single-parent implementation), (b) conflict during cherry-pick with `--complete-conflict` resume, (c) zero-diff plan range behavior (whether commit is produced or operation is a no-op), and (d) CI/CD gate still runs/amends in squash mode.  
- Include unit tests for the new VCS helpers (range calculation, soft reset) guarding against incorrect state after conflicts.

## Risks & Unknowns
- Git state management: cherry-pick/rebase conflicts may leave the repo in CHERRYPICK/REBASE states; resume logic must cleanly detect and exit without corrupting history.  
- Empty or no-op ranges may change behavior (commit vs no commit); need to align with current expectations.  
- Plan branches with merges or non-linear history might need explicit sequencing decisions (flatten order) to avoid surprises.  
- Agent auto-resolve tooling currently assumes merge-conflict contexts; it may need adjustment for cherry-pick conflict markers.

## Testing & Verification
- Integration: default squash merge on a plan with two commits produces exactly one implementation commit plus the merge commit, with the implementation tree matching the plan tip and parent pointing to the pre-merge target.  
- Integration: `--no-squash` path unchanged.  
- Integration: conflict scenario during cherry-pick allows `--complete-conflict` to finalize the implementation commit and merge.  
- Integration: CI/CD gate still runs against the squashed implementation commit; auto-fix amends it when enabled.  
- Unit: VCS helpers compute ranges correctly, handle zero commits, and leave the repo clean after soft-squash or report conflicts with sentinel-ready metadata.

## Notes
- Narrative change this run: drafted the plan only; no repo files were modified.
