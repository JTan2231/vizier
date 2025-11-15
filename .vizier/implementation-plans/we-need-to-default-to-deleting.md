---
plan: we-need-to-default-to-deleting
branch: draft/we-need-to-default-to-deleting
status: draft
created_at: 2025-11-15T18:14:15Z
spec_source: inline
---

## Operator Spec
we need to default to deleting the branch on a successful vizier merge call, and add a flag to abstain from doing so if the user desires

## Implementation Plan
## Overview
`vizier merge` currently leaves the draft branch intact unless operators pass `--delete-branch`. The operator spec asks us to invert that posture so successful merges clean up their draft branches automatically, while still allowing humans to keep the branch when needed. Shipping this change reduces lingering worktrees/branches (which already create friction in the documented draft→approve→merge flow) and better matches the compliance-first snapshot in `.vizier/.snapshot`.

## Execution Plan
1. **Flip the CLI-facing switch to “delete by default.”**
   - Update `vizier-cli/src/main.rs` so `MergeCmd` exposes a `--keep-branch` (or similarly named) flag whose help text explicitly states that branches are deleted unless this flag is present.
   - Keep compatibility by either aliasing the old `--delete-branch` name (marked deprecated) or by surfacing a clear error that nudges operators toward the new flag.
   - Ensure the `--help` output and clap-derived docs show the new default so scripts and humans see the change without reading code.
2. **Thread the new semantics through merge execution.**
   - In `resolve_merge_options` populate `MergeOptions.delete_branch` with `true` by default and flip it to `false` when the opt-out flag is supplied.
   - Audit `run_merge`/`finalize_merge` in `vizier-cli/src/actions.rs` to confirm that branch deletion now happens on the default path, that it still checks `graph_descendant_of` before deleting, and that the CLI output remains informative (“Deleted draft/foo after merge” versus “Skipping deletion…”).
   - Confirm pending-conflict resume (`try_complete_pending_merge`) reuses the new default so reruns after manual conflict resolution still delete unless suppressed.
3. **Update operator-facing documentation and workflows.**
   - Refresh the README Core Commands entry for `vizier merge` and `docs/workflows/draft-approve-merge.md` so they explain the new default behavior, mention the opt-out flag, and keep the flag tables accurate.
   - If AGENTS.md or any other reference mentions the `--delete-branch` flag explicitly, update or cross-link it so multi-agent operators understand the cleanup expectation.
4. **Extend integration coverage.**
   - Update existing merge integration tests (`tests/src/lib.rs`, `tests/src/main.rs`) to drop manual `--delete-branch` usage where it becomes redundant and assert that the branch disappears without extra flags.
   - Add a new test that runs `vizier merge <plan> --keep-branch` (or the chosen flag) and verifies the branch still exists afterward, ensuring we don’t regress the opt-out path.

## Risks & Unknowns
- Shell scripts or docs that still pass `--delete-branch` could break if we remove the flag outright; retaining it as an alias (while noting it’s now redundant) mitigates that compatibility risk.
- Automatically deleting branches after merge assumes the merge commit includes the latest draft tip; `finalize_merge` already checks this, but we should double-check Git edge cases (e.g., forced pushes on the draft branch) so we don’t delete a branch that still has unmerged commits.
- We assume no remote branch deletion is required; if operators expect remote cleanup, they still need to run `git push --delete`. Highlighting that in docs might be out of scope but worth keeping in mind.

## Testing & Verification
- `cargo test -p vizier-cli` (or the targeted merge integration tests) to ensure clap parsing and new flag wiring compile.
- Integration test: run `vizier merge remove-plan --yes` inside `tests/src/lib.rs` harness and assert `git rev-parse --verify draft/remove-plan` fails afterward.
- Integration test: run `vizier merge remove-plan --yes --keep-branch` and assert the branch remains, proving the opt-out works.
- Manual sanity: invoke `vizier merge foo --help` to confirm the help text explains the new default and opt-out flag.

## Notes
- Drafted a plan to flip `vizier merge` to delete draft branches by default while adding a keep-branch escape hatch.
