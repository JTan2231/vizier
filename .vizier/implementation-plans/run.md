---
plan_id: pln_72c59c7a1460422b856d01c6f043ae9e
plan: run
branch: draft/run
---

## Operator Spec
# Feature Spec: Flagship `vizier run` Alias Outcomes (`RUN`)

## Status

- Proposed implementation spec.
- Scope date: 2026-02-20.
- This spec defines user-outcome-first integration expectations for built-in `vizier run` aliases.

## Purpose

Define a single flagship integration contract for each built-in alias so a user can answer:

"What should I expect when I run this command?"

The goals are:

1. User-visible outcomes over internal node mechanics.
2. One flagship end-to-end test per alias (`draft`, `approve`, `merge`, `develop`, `commit`).
3. Stable CLI-level guarantees backed by integration coverage.

## Decision

`tests/src/run.rs` MUST include one flagship case per built-in alias that validates final, user-visible outcomes after `vizier run <alias> --follow`.

These flagship cases are additive to existing queue-time/runtime-unit coverage and existing stage smoke tests.

## Scope

In scope:

- Integration assertions for end-state outcomes of `vizier run draft|approve|merge|develop|commit`.
- Shared "flagship test contract" for setup, execution, and assertion style.
- Clear acceptance criteria that map command -> observable repository result.

Out of scope:

- Replacing fine-grained runtime tests for node routing/retry internals.
- Changing workflow semantics themselves.
- Reintroducing removed wrapper commands.

## Flagship Test Contract

Each flagship alias test MUST:

1. Execute the public command form (`vizier run <alias> ... --follow --format json`).
2. Assert terminal success from the run payload and root job status.
3. Assert command alias metadata on root records (`command_alias == <alias>`).
4. Assert at least one user-visible repository outcome (branch/file/commit effect).
5. Prefer branch/file/commit assertions over node-ID-specific assertions.

Each flagship alias test SHOULD avoid relying on implementation-only internals unless no user-visible equivalent exists.

## Alias Outcome Contracts

### `vizier run draft`

Given a spec input and slug, the user should expect:

1. A draft plan document exists on the draft branch for that slug.
2. The plan doc contains expected plan structure and generated body text.
3. The draft stage results in a successful terminal run.

Minimum observable assertions:

- Draft branch exists.
- `.vizier/implementation-plans/<slug>.md` exists on the draft branch tip.
- Plan doc includes `## Implementation Plan` and non-empty generated content.

### `vizier run approve`

Given an existing draft plan/branch, the user should expect:

1. Approval-stage execution completes successfully when stop condition passes.
2. The result is merge-ready stage state (approval token/output produced for downstream merge).
3. Approve run can succeed with explicit branch input and with implicit branch derivation from slug.

Minimum observable assertions:

- `vizier run approve ... --follow` terminal success.
- Downstream `merge` is unblocked by stage-token dependency after approve success.

### `vizier run merge`

Given an approved draft branch and merge inputs, the user should expect:

1. Changes land on the target branch.
2. Merge message/subject reflects provided merge intent.
3. Plan-doc lifecycle behavior is enforced (embedded content + source cleanup behavior).
4. Conflict flows preserve recovery sentinel state when blocked.

Minimum observable assertions:

- Target branch working tree/file reflects merged content.
- Head commit subject/message matches merge inputs.
- Conflict sentinel exists when merge blocks on conflict.

### `vizier run develop`

Given one command invocation, the user should expect:

1. `draft -> approve -> merge` chain executes end-to-end.
2. The final target branch contains the merged outcome.
3. The run behaves like an integrated "ship this change" path, not just composed enqueue metadata.

Minimum observable assertions:

- End-to-end `develop --follow` terminal success.
- Target branch includes expected change from draft input.
- Chain-level outcome is observable without inspecting node internals.

### `vizier run commit`

Given tracked modifications, the user should expect:

1. A commit is created on the current branch.
2. Commit message is sourced from commit workflow prompt/agent output.
3. Commit includes tracked changes and leaves expected clean/staged state.

Minimum observable assertions:

- New HEAD commit exists after run.
- Commit subject/message is non-empty and matches configured/mock agent response shape.
- Target tracked file changes are present in committed tree.

## Required Test Additions

Add or update the following integration cases in `tests/src/run.rs`:

1. `test_run_flagship_draft_user_outcome`
2. `test_run_flagship_approve_user_outcome`
3. `test_run_flagship_merge_user_outcome`
4. `test_run_flagship_develop_user_outcome`
5. `test_run_flagship_commit_user_outcome`

Guidance:

- Reuse existing fixture helpers (`seed_plan_branch`, `run_stage_approve_follow`, manifest/job helpers).
- Keep mock-agent deterministic.
- Keep assertions user-facing first; retain node-level checks only as secondary diagnostics.

## Acceptance Criteria

1. Each built-in alias has one flagship integration test that passes locally.
2. Each flagship test asserts terminal success plus at least one repository-visible end result.
3. `develop` coverage is upgraded from compose/enqueue-only to end-to-end outcome coverage.
4. `commit` coverage includes direct alias invocation (`vizier run commit`), not only file-selector commit workflows.
5. Existing fine-grained tests remain intact and complementary.

## References

- `tests/src/run.rs`
- `tests/src/fixtures.rs`
- `.vizier/config.toml`
- `.vizier/workflows/draft.hcl`
- `.vizier/workflows/approve.hcl`
- `.vizier/workflows/merge.hcl`
- `.vizier/workflows/commit.hcl`
- `.vizier/develop.hcl`

## Implementation Plan
## Overview
This plan adds one flagship, user-outcome integration test per built-in `vizier run` alias in `tests/src/run.rs`: `draft`, `approve`, `merge`, `develop`, and `commit`. The goal is to make reviewer approval hinge on user-visible repository outcomes (branch/file/commit effects) rather than node-internal mechanics, while preserving the reduced CLI surface where `run` is the only enqueue front door.

Users impacted are operators relying on alias workflows as stable “what happens when I run this” contracts. This is needed now because current coverage is strong on queue/runtime mechanics and stage smoke behavior, but does not yet provide one explicit end-to-end outcome contract per alias.

Snapshot tension to track: `.vizier/narrative/snapshot.md` frames active stage aliasing around `draft`/`approve`/`merge` (plus composed `develop`), while the spec requires `commit` as a flagship built-in alias outcome. Reconciliation: keep `commit` strictly as `vizier run commit` alias coverage (no wrapper command resurrection), and treat this as additive run-surface contract coverage.

## Execution Plan
1. Lock the flagship contract in test scaffolding in `tests/src/run.rs` using existing helpers from `tests/src/run.rs` and `tests/src/fixtures.rs`.
Add a shared assertion pattern that each flagship test uses: invoke `vizier run <alias> ... --follow --format json`, assert payload terminal success, read root job record from `root_job_ids`, assert root `status == succeeded`, and assert `/metadata/command_alias == <alias>`. Keep node-ID assertions secondary only.

2. Ensure deterministic alias resolution and mock-agent behavior for all flagship aliases.
Use repository alias mappings from `.vizier/config.toml` and existing stage helper patterns; if test-local config overrides are used, include `commit` alias wiring so `vizier run commit` resolves as alias (not selector fallback), preserving `command_alias` metadata expectations.

3. Add `test_run_flagship_draft_user_outcome`.
Execute `vizier run draft ... --follow --format json` with unique slug/spec input. Assert draft branch exists, `.vizier/implementation-plans/<slug>.md` exists on draft branch tip, plan doc contains `## Implementation Plan`, and generated body text is non-empty/user-visible.

4. Add `test_run_flagship_approve_user_outcome`.
Seed draft plan branch, run `vizier run approve ... --follow --format json`, assert terminal success and alias metadata, and assert downstream merge dependency is unblocked after approve (stage token effect observable via merge readiness/success path). Include both explicit branch input and implicit branch derivation from slug in this flagship case.

5. Add `test_run_flagship_merge_user_outcome`.
Prepare approved source branch and target branch inputs, run `vizier run merge ... --follow --format json`, assert merged content lands on target branch and commit subject/message reflects merge intent, and assert plan-doc lifecycle behavior (embedded content and source cleanup) is visible. Include blocked conflict sub-scenario in the same flagship case and assert sentinel persistence at `.vizier/tmp/merge-conflicts/<slug>.json`.

6. Add `test_run_flagship_develop_user_outcome`.
Invoke `vizier run develop ... --follow --format json` using real composed flow from `.vizier/develop.hcl`; assert terminal success and alias metadata, then assert final target branch contains expected merged change as integrated draft→approve→merge behavior (not just compose/enqueue metadata).

7. Add `test_run_flagship_commit_user_outcome`.
Create tracked modifications, run `vizier run commit --follow --format json`, assert terminal success and alias metadata, assert new HEAD commit exists with non-empty message matching deterministic mock-agent response shape, and assert tracked file changes are present in committed tree with expected post-run index/worktree state.

8. Keep existing coverage complementary.
Retain current fine-grained runtime/stage tests in `tests/src/run.rs`; flagship tests are additive and user-outcome-first, not replacements for node-routing/retry/runtime matrix coverage.

## Risks & Unknowns
- `commit` alias positioning is the main spec-vs-snapshot tension. Mitigation: keep implementation and assertions strictly on `run` alias behavior, with no top-level command surface expansion.
- Follow-mode tests can flake under scheduler timing if assertions are made before root jobs settle. Mitigation: always assert against follow JSON terminal payload plus persisted root job record.
- Existing helper config overrides may omit `commit`, which would break alias-metadata assertions by resolving through selector path. Mitigation: align test config overrides with required alias set.
- Merge conflict checks are stateful and can be sensitive to fixture contamination. Mitigation: isolate each scenario with unique slugs/branches and fresh `IntegrationRepo` instances.

## Testing & Verification
- Run targeted flagship coverage in `tests/src/run.rs` and confirm all five new tests pass.
- Verify each flagship test emits these acceptance signals:
`outcome == workflow_run_terminal`, `terminal_state == succeeded` (except intentional blocked conflict sub-scenario inside merge flagship), root job `status` is terminal as expected, and root metadata has `command_alias` matching invoked alias.
- Validate repository-visible effects per alias:
draft plan doc on draft branch; approve unblocks downstream merge path; merge lands content and preserves conflict sentinel on blocked path; develop produces end-to-end merged outcome; commit creates new HEAD commit with expected message/content.
- Run full validation gate required by repo policy: `./cicd.sh` (plus `cargo test --all --all-targets` as part of that gate).

## Notes
- Primary orientation surfaces for reviewers: `tests/src/run.rs`, `tests/src/fixtures.rs`, `vizier-cli/src/actions/run.rs`, `vizier-cli/src/workflow_templates.rs`, `vizier-core/src/jobs/mod.rs`, `.vizier/config.toml`, `.vizier/workflows/draft.hcl`, `.vizier/workflows/approve.hcl`, `.vizier/workflows/merge.hcl`, `.vizier/workflows/commit.hcl`, `.vizier/develop.hcl`.
- Legacy wrapper-removal behavior remains untouched; this effort only strengthens `vizier run` alias outcome contracts.
