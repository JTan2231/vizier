---
plan: list
branch: draft/list
status: draft
created_at: 2025-11-16T01:21:16Z
spec_source: inline
---

## Operator Spec
there should be a basic list command which lists outstanding drafts. this is essentially already as part of the approve command, but really it should be standalone instead

## Implementation Plan
## Overview
Operators currently have to run `vizier approve --list` (see README.md:91 and docs/workflows/draft-approve-merge.md) to see which `draft/<slug>` branches remain pending. The operator spec requests a standalone “list outstanding drafts” entry point so review and orchestration steps don’t depend on the approval workflow. Creating a dedicated `vizier list` command reduces the flag sprawl called out in `.vizier/todo_draft_approve_merge_config_surface.md`, makes it easier for agents/humans to inspect backlog state, and keeps the approve command focused on actually applying a plan.

## Execution Plan
1. **Design the standalone CLI surface**
   - Introduce a `List` subcommand in `vizier-cli/src/main.rs` with a `ListCmd` struct that mirrors the existing target-override story (optional `--target`, defaults to `detect_primary_branch()`).
   - Document the command in its Clap docstring as “List pending implementation-plan branches that are ahead of the target branch.”
   - Route `Commands::List` through `run_list` in `vizier-cli/src/actions.rs`.

2. **Reuse and harden the existing listing logic**
   - Move `list_pending_plans` (and `resolve_target_branch`) into a shared helper block that both `run_list` and any compatibility shim can call.
   - Implement `run_list` as a thin wrapper around `list_pending_plans`, handling errors via the same `Result` flow the other commands use.
   - Keep the output format (`plan=... branch=... created=... summary="..."` plus the fallback “No pending draft branches”) identical so existing scripts keep working.

3. **Handle `vizier approve --list` compatibility**
   - Remove public documentation of `--list` from `ApproveCmd`, but keep a hidden/deprecated `--list` flag that simply calls `list_pending_plans` and prints a warning directing users to `vizier list`.
   - Update `ApproveCmd`/`ApproveOptions` so the standard approval path no longer has to branch on `list_only`, simplifying argument validation.
   - Ensure the compatibility path terminates early (before Codex/backend checks) so legacy scripts don’t fail when Codex isn’t configured.

4. **Documentation updates**
   - README “Core Commands” section: add a bullet for `vizier list` describing its behavior/flags, and remove the `--list` flag description from the `vizier approve` entry to avoid conflicting guidance.
   - docs/workflows/draft-approve-merge.md: add mention of `vizier list` in the high-level workflow (likely after the draft section) and update the “Flags to remember” list to point to the new command instead of `vizier approve --list`.
   - Confirm no other docs reference `approve --list`; if they do, adjust them to mention `vizier list` and, if relevant, call out the deprecation.

5. **Integration and regression tests**
   - Update tests in `tests/src/lib.rs` and `tests/src/main.rs` that currently invoke `vizier approve --list` to use `vizier list` (or to exercise both the new command and the compatibility flag if we want to assert the warning).
   - Add/adjust assertions so tests continue to confirm the command output includes the expected plan slug before approval and emits “No pending draft branches” after merge.
   - Run the integration test suite (`cargo test -p tests`) to ensure the new command wires cleanly into the existing workflow.

## Risks & Unknowns
- **Naming scope**: `vizier list` implicitly targets implementation plans; if future list-style commands are needed (e.g., TODOs), we may need to revisit naming or add subcommands. Documenting the focus in help text should mitigate confusion for now.
- **Backward compatibility**: Some operators may rely on `vizier approve --list`; keeping a hidden alias plus a warning reduces breakage, but we need to make sure the warning isn’t treated as an error in scripts (stick to stderr).
- **Target detection failures**: `list` still depends on `detect_primary_branch`; if auto-detection fails, users must pass `--target`. We should mention that in the help text/README entry so the behavior is unsurprising.

## Testing & Verification
- `cargo test -p tests test_approve_merges_plan test_merge_removes_plan_document` (and any other suites touched) to ensure the integration harness exercises the new command before/after approvals.
- Manual `vizier list` run in a repo with multiple `draft/*` branches to confirm output formatting and the fallback message when nothing is pending.
- Optional smoke test for the hidden `vizier approve --list` flag (if retained) to confirm it prints the deprecation warning yet exits successfully.

## Notes
- Once operators adopt `vizier list`, we can schedule full removal of the approve-flag shim in a future compliance sweep; document that timeline when announced.
