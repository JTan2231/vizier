---
plan: truer-squashing
branch: draft/truer-squashing
status: draft
created_at: 2025-11-21T19:06:01Z
spec_source: inline
---

## Operator Spec
Current: Squashed merge cherry-picks/squashes the plan into one implementation commit, then writes a two-parent merge (parent2 = draft tip), so plan commits stay visible off the merge.
Desired: Default squashed merge yields only the implementation commit plus a merge commit on target; no draft-branch ancestry is reachable from the merged graph.
Change: Drop parent2 in the squashed path (single-parent merge), and update conflict/resume handling and docs/tests that currently expect the two-parent merge shape.

## Implementation Plan
## Overview
- Change default squashed merges so the merge commit has a single parent (the squashed implementation commit), severing draft-branch ancestry from the target history while keeping legacy `--no-squash` unchanged.
- Impacts operators consuming merge history, branch-cleanup logic, conflict/resume flows, CI/CD gate runs, and docs/tests that currently describe two-parent squashed merges.
- Snapshot/docs currently describe parents `[implementation, draft/<slug>]`; we will reconcile that narrative with the new single-parent default.

## Execution Plan
1) Map current squashed-merge flow and dependencies: trace `vizier-cli/src/actions.rs` (squash path: `execute_squashed_merge`, `finalize_squashed_merge_from_head`, `try_complete_pending_merge`) and `vizier-core/src/vcs.rs` (`MergeReady`, `commit_ready_merge`, `commit_squashed_merge`, branch-deletion checks) to list all places assuming a second parent.
2) Rework squashed merge commit creation to be single-parent: adjust the squashed path to commit the merge with only the implementation commit as its parent (e.g., by introducing/using a single-parent helper instead of `commit_ready_merge`), preserving `--no-squash` legacy behavior. Ensure CI/CD gate still runs against the squashed implementation commit before the merge is written.
3) Align conflict/replay/resume handling: update merge-conflict state, replay metadata, and `--complete-conflict` paths so they no longer expect a merge tip that references the draft branch. Confirm agent/manual resolution still produces the implementation commit, then finalizes the single-parent merge cleanly.
4) Adjust branch deletion and post-merge bookkeeping: change the “safe to delete draft branch” detection and outcome messaging to work without draft ancestry while still preventing accidental deletion when the plan wasn’t applied. Keep PlanSlug metadata and session logging unaffected.
5) Update docs and narrative references: refresh README and `docs/workflows/draft-approve-merge.md` (and snapshot/TODO narratives) to describe the new default topology and the legacy `--no-squash` path. Note the no-ancestry guarantee and any effects on conflict/resume guidance.
6) Refresh tests and add coverage: update existing merge integration tests (squash default, replay, zero-diff, conflict replay) to assert single-parent merge commits, keep no-squash expectations intact, and add a check that merged history is not a descendant of the draft tip while branch deletion still behaves as intended.

## Risks & Unknowns
- Branch deletion safeguards currently rely on ancestry; need a robust alternative signal to avoid deleting an unmerged draft branch.
- Conflict-resume metadata may hide subtle assumptions about parent counts; must validate `--complete-conflict` for squashed merges still works.
- CI/CD gate and auto-fix flows must continue to amend the right commit after the topology change.

## Testing & Verification
- Targeted integration tests: `test_merge_default_squash_adds_implementation_commit`, `test_merge_squash_replays_plan_history`, `test_merge_squash_allows_zero_diff_range`, `test_merge_squash_replay_respects_manual_resolution_before_finishing_range`, `test_merge_no_squash_matches_legacy_parentage`, plus CI/CD gate tests to ensure no regression.
- Manual git inspection on a sample merge: verify merge commit parent_count=1, implementation commit contains plan changes, and `graph_descendant_of(<merge>, <draft-tip>)` is false while branch deletion behaves per flags.

## Notes
- Snapshot and any TODO threads mentioning two-parent squashed merges must be updated alongside the code to keep canonical narrative in sync.
