# Merge Command Feature Spec (`vizier merge`)

## Status

- Scope: document current behavior of the existing `vizier merge` command.
- This is not a redesign proposal; it is the implementation contract as shipped.

## Purpose

`vizier merge` lands an approved plan branch (`draft/<slug>` by default) onto a target branch, preserves merge metadata in commit history, enforces merge-time gates, and provides a resumable conflict workflow.

## Command Contract

## CLI shape

```bash
vizier merge <plan> [--target <branch>] [--branch <branch>] [--yes]
             [--keep-branch] [--note <text>]
             [--squash|--no-squash] [--squash-mainline <n>]
             [--auto-resolve-conflicts|--no-auto-resolve-conflicts]
             [--complete-conflict]
             [--cicd-script <path>] [--cicd-retries <n>]
             [--auto-cicd-fix|--no-auto-cicd-fix]
             [--after <job-id> ...]
```

## Runtime mode

- `merge` is scheduler-backed and enqueues workflow jobs.
- In non-TTY mode, `--yes` is required.
- In TTY mode, confirmation is prompted if `--yes` is not provided.
- Global `--no-commit` is rejected for `merge`.

## Primary inputs

- `plan`: required plan slug.
- `target`: optional target branch; defaults to detected primary branch.
- `branch`: optional plan branch override; defaults to `draft/<plan>`.
- `keep-branch`: disables default post-merge draft-branch deletion.
- `note`: optional operator note added to merge commit message body when commit template allows it.

## Configuration Contract

`vizier merge` resolves config from merged config layers plus CLI overrides:

- `[merge].squash` (default `true`): default history mode.
- `[merge].squash_mainline` (default `none`): parent index for replaying merge commits in squash mode.
- `[merge.conflicts].auto_resolve` (default `false`): default conflict auto-resolution posture.
- `[merge.cicd_gate].script` (default `none`): optional merge gate script.
- `[merge.cicd_gate].auto_resolve` (default `false`): CI/CD auto-remediation toggle.
- `[merge.cicd_gate].retries` (default `1`): CI/CD remediation budget.
- `[commits.implementation]`: implementation-commit template for squash mode.
- `[commits.merge]`: merge-commit template and plan embedding mode.

Workflow template policy is authoritative at runtime for gate/conflict wiring. If template policy differs from CLI/config intent, effective behavior follows the resolved merge template.

## Execution Model

## Phase 1: Resolve context and preflight

1. Resolve plan slug, source branch, and target branch.
2. Resolve merge workflow template and effective gate/conflict policy.
3. Validate required branches exist.
4. If target already contains source tip, exit as no-op with "Plan already merged".
5. Attempt to resume any pending Vizier-managed conflict state before starting a new integration.

## Phase 2: Plan branch refresh

1. Create temporary plan worktree.
2. Remove `.vizier/implementation-plans/<slug>.md` on the plan branch before merge.
3. Run narrative refresh on the plan branch and commit resulting narrative updates.
4. Preserve worktree on refresh failure for debugging; cleanup on success.

## Phase 3: Integration

### Default (`squash`) path

1. Cherry-pick plan commits onto target-side base.
2. Create one implementation commit (`feat: apply plan <slug>` by default).
3. Run merge CI/CD gate before final merge commit.
4. Create final merge commit (`feat: merge plan <slug>` by default), embedding plan metadata/document block per commit config.

### Legacy (`--no-squash`) path

1. Create regular merge commit from plan branch.
2. Run merge CI/CD gate after merge commit creation.

## Phase 4: Finalize

1. Optionally delete plan branch (default behavior) with safety checks.
2. Keep branch when `--keep-branch` is set.
3. Optionally push to origin when global `--push` was provided.
4. Emit outcome block with plan, target, merge commit, and gate summary fields.

## Merge History Rules

- Squash mode requires mainline disambiguation when plan history contains merge commits.
- If merge commits exist and no explicit/inferable mainline is usable, merge fails with `--squash-mainline` guidance.
- Octopus merge history is rejected in squash mode; operator must use `--no-squash` or rewrite history.
- Zero-diff squash ranges are allowed.

## Conflict Lifecycle

## Sentinel file

- Path: `.vizier/tmp/merge-conflicts/<slug>.json`
- Contains source/target refs, expected HEAD, merge messages, and squash replay metadata when applicable.

## Manual resolution flow

1. Merge/cherry-pick conflict occurs.
2. Sentinel is written.
3. Operator resolves conflicts and stages files.
4. Operator runs `vizier merge <slug> --complete-conflict`.
5. Merge finalizes and sentinel is removed.

## Auto-resolution flow

- Enabled by `--auto-resolve-conflicts` or `[merge.conflicts].auto_resolve = true` (subject to template policy).
- Requires an agent-capable merge-conflict backend.
- On backend failure, merge falls back to manual conflict handling and leaves sentinel for resume.

## Resume guards (`--complete-conflict`)

Resume is blocked when:

- no Vizier-managed pending sentinel exists for the slug,
- current checkout is not the expected target branch,
- Git is no longer in expected merge/cherry-pick state for pending context,
- conflict markers remain unresolved/staged incompletely,
- HEAD drift invalidates pending replay assumptions.

## CI/CD Gate Behavior

- Gate is skipped when no script is configured.
- Gate failure blocks merge completion.
- With auto-remediation enabled and template retry path wired, merge attempts backend fixes up to retry budget.
- In squash mode, remediation amends implementation commit when fixes are produced.
- Gate events are recorded in session operations (`kind = cicd_gate`, `scope = merge`) with status, attempts, output snippets, and fix metadata.

## Observability and Artifacts

- Scheduler job metadata captures template id/version/node/capability and gate labels.
- Merge outcome summary includes `Outcome`, `Plan`, `Target`, and `Merge commit`.
- Merge outcome summary includes `CI/CD script`, `Gate attempts`, and `Gate fixes` when applicable.
- Conflict sentinel is durable until merge completion or invalidation cleanup.

## Acceptance Criteria (Current Behavior)

1. Merge requires `--yes` in non-TTY scheduler mode.
2. Default merge path is squash and yields implementation+merge commits (single-parent merge commit).
3. `--no-squash` preserves legacy merge parentage behavior.
4. Plan document is removed from merged history and merge commit includes plan metadata block.
5. Squash merge with merge-history requires `--squash-mainline`; octopus merge history fails in squash mode.
6. CI/CD gate runs when configured, blocks on failure, and records operations.
7. CI/CD auto-fix retries and records fix metadata when enabled and backend-capable.
8. Conflicts create sentinel state; `--complete-conflict` resumes only under valid state/branch/head conditions.
9. Auto conflict resolution can resolve directly or during resume; backend errors surface clearly and preserve manual recovery path.
10. Successful merge deletes draft branch by default unless `--keep-branch` is set.

## Implementation References

- `vizier-cli/src/actions/merge.rs`
- `vizier-cli/src/cli/args.rs`
- `vizier-cli/src/cli/resolve.rs`
- `vizier-cli/src/actions/workflow_runtime.rs`
- `vizier-kernel/src/config/defaults.rs`
- `tests/src/merge.rs`

## Companion Prompt Doc

- `specs/MERGE_PROMPTS.md`
