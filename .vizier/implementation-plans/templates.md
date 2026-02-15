---
plan_id: pln_7e6d2e5c5e5a432e818d7197cde0bc44
plan: templates
branch: draft/templates
---

## Operator Spec
# Feature Spec: Primitive Stage Templates (`draft` / `approve` / `merge`)

## Status

- Proposed implementation decree.
- Scope date: 2026-02-15.
- This spec defines the required implementation posture for stage workflows after command deprecation.

## Purpose

Replace deprecated command-era stage execution with canonical repo-local workflow templates executed only through:

- `vizier run <flow>`
- `vizier jobs ...`

No command-specific runtime wrappers are part of this design.

## Decision

The repository MUST implement `draft`, `approve`, and `merge` as template DAGs using canonical `uses` primitives (`cap.env.*`, `cap.agent.invoke`, `control.*`).

Legacy `vizier.*` template labels are not permitted.

## Non-Goals

- Reintroducing top-level `vizier draft|approve|merge` commands.
- Preserving command-era UX/output parity.
- Preserving command-era prompt-envelope behavior.

## Required CLI Surface

Stage orchestration is performed with alias-driven `vizier run` entries and scheduler primitives:

- `vizier run draft ...`
- `vizier run approve ...`
- `vizier run merge ...`
- `vizier jobs list|show|tail|attach|approve|reject|retry|cancel|gc ...`

`vizier run` queue-time controls (`--set`, `--after`, `--require-approval`, `--follow`) remain authoritative.

## Template Contract

All stage templates MUST satisfy canonical validation in `vizier-kernel/src/workflow_template.rs`:

- Canonical-only `uses` labels.
- Capability-contract checks for prompt/invoke, apply/stop-condition loop, and merge gate wiring.
- Queue-time rejection on unresolved placeholders or invalid typed coercions.

## Stage Definitions

### Draft Stage (`template.stage.draft@v2`)

Intent: produce plan artifacts on an isolated draft branch using primitives only.

Required node chain:

1. `worktree_prepare` (`cap.env.builtin.worktree.prepare`)
2. `resolve_prompt` (`cap.env.builtin.prompt.resolve` or `cap.env.shell.prompt.resolve`)
3. `invoke_agent` (`cap.agent.invoke`)
4. `persist_plan` (`cap.env.builtin.plan.persist`)
5. `stage_commit` (`cap.env.builtin.git.stage_commit`) unless commit mode is intentionally disabled
6. `worktree_cleanup` (`cap.env.builtin.worktree.cleanup`)
7. `terminal` (`control.terminal`)

Required contracts:

- `resolve_prompt` MUST produce exactly one `custom:prompt_text:<key>` artifact.
- `invoke_agent` MUST consume exactly one prompt artifact.
- `persist_plan` MUST receive valid `spec_source` and `spec_text`/`spec_file` inputs.
- Plan durability must be represented through `plan_branch` and `plan_doc` artifacts.

### Approve Stage (`template.stage.approve@v2`)

Intent: apply plan implementation in isolated worktree, commit changes, and enforce optional stop-condition retries through template wiring.

Required node chain:

1. `worktree_prepare` (`cap.env.builtin.worktree.prepare`)
2. `resolve_prompt` (`cap.env.builtin.prompt.resolve` or `cap.env.shell.prompt.resolve`)
3. `invoke_agent` (`cap.agent.invoke`)
4. `stage_commit` (`cap.env.builtin.git.stage_commit`)
5. `stop_gate` (`control.gate.stop_condition`) when stop-condition is configured
6. `worktree_cleanup` (`cap.env.builtin.worktree.cleanup`)
7. `terminal` (`control.terminal`)

Required stop-condition wiring:

- `stage_commit.on.succeeded -> stop_gate`
- `stop_gate.retry.mode = until_gate` when retrying is enabled
- `stop_gate.on.failed -> stage_commit` for retry loop closure

Required contracts:

- Stop-condition gate cardinality and routing MUST pass capability validation.
- Approval gating, when needed, is expressed through scheduler/root approval controls (template node or `--require-approval`).

### Merge Stage (`template.stage.merge@v2`)

Intent: integrate plan branch into target branch and enforce conflict/CI/CD control policies using canonical merge primitives.

Required integration node:

- `merge_integrate` (`cap.env.builtin.git.integrate_plan_branch`)

Optional/conditional control nodes:

- `merge_conflict_resolution` (`control.gate.conflict_resolution`)
- `merge_gate_cicd` (`control.gate.cicd`)
- `merge_sentinel_write` (`cap.env.builtin.merge.sentinel.write`) and `merge_sentinel_clear` (`cap.env.builtin.merge.sentinel.clear`) when explicit sentinel management is modeled
- `terminal` (`control.terminal`)

Required merge contracts:

- Conflict gate is targeted from integrate `on.blocked` when conflict flow is enabled.
- CI/CD gate is targeted from integrate `on.succeeded` when CI/CD gating is enabled.
- Auto-resolve flows MUST define valid retry-loop closure per canonical validator rules.

## Repo-Local Layout

Repository-local stage templates MUST live under:

- `.vizier/workflow/draft.toml`
- `.vizier/workflow/approve.toml`
- `.vizier/workflow/merge.toml`

Composed flows (for example `develop`) MAY import/link these stage templates from `.vizier/develop.toml`.

## Alias Mapping

Repo config SHOULD map stage aliases through `[commands]` to file selectors, for example:

- `draft = "file:.vizier/workflow/draft.toml"`
- `approve = "file:.vizier/workflow/approve.toml"`
- `merge = "file:.vizier/workflow/merge.toml"`

Legacy wrapper/template-scope indirection is compatibility-only and not required for this design.

## Migration Requirements

1. Replace legacy `vizier.*` `uses` labels in repo-local stage templates with canonical labels.
2. Preserve stage semantics through explicit DAG edges and gate/retry policies.
3. Validate migration using queue-time template validation and runtime scheduler execution.

## Acceptance Criteria

1. `vizier run draft ...` enqueues and executes a canonical primitive DAG that persists `plan_branch` + `plan_doc` artifacts.
2. `vizier run approve ...` enqueues and executes a canonical primitive DAG that runs agent implementation and `git.stage_commit`, with optional stop-condition retry loop enforced by template wiring.
3. `vizier run merge ...` enqueues and executes a canonical primitive DAG using `git.integrate_plan_branch` with optional conflict and CI/CD gate nodes.
4. Any template containing `uses = "vizier.*"` is rejected at queue time.
5. `vizier jobs approve|retry|cancel|tail|attach` operates correctly against stage-run jobs without command-specific wrappers.

## Testing Requirements

Integration coverage MUST include:

- Canonical stage-template enqueue and execution smoke tests under `tests/src/run.rs`.
- Queue-time rejection for legacy `vizier.*` labels.
- Stop-condition retry-loop behavior for approve-style templates.
- Conflict and CI/CD gate behavior for merge-style templates.

## References

- `docs/user/workflows/alias-run-flow.md`
- `docs/user/workflows/stage-execution.md`
- `docs/dev/scheduler-dag.md`
- `docs/dev/vizier-material-model.md`
- `vizier-kernel/src/workflow_template.rs`
- `vizier-cli/src/jobs.rs`
- `specs/DRAFT.md`
- `specs/APPROVE.md`
- `specs/MERGE.md`

## Implementation Plan
## Overview
This work formalizes `draft`, `approve`, and `merge` as canonical primitive workflow templates executed only through `vizier run` and operated through `vizier jobs`, matching the post-wrapper CLI posture. The primary users are operators running stage workflows and reviewers/auditors who need deterministic queue-time validation and runtime observability. This is needed now because the wrapper command family is already removed in the snapshot, so stage behavior must be fully anchored to repo-local templates, alias mapping, and scheduler primitives.

## Execution Plan
1. Establish the stage entrypoint contract on `run`/`jobs` only.  
Confirm repo-local alias mapping in `.vizier/config.toml` points `draft`, `approve`, and `merge` to `.vizier/workflow/{draft,approve,merge}.toml`, and ensure no command-era wrapper assumptions remain in CLI/help/man/docs surfaces.  
Acceptance signal: `vizier run draft|approve|merge` resolves cleanly; removed wrappers still fail via standard unknown-subcommand behavior.

2. Align stage templates to the required canonical DAG contracts.  
Audit and update `.vizier/workflow/draft.toml`, `.vizier/workflow/approve.toml`, and `.vizier/workflow/merge.toml` against the spec-required node chains, canonical `uses` IDs, prompt artifact producer/consumer contracts, approve stop-gate retry closure, and merge conflict/CI/CD gate routing.  
Dependency: step 1 alias contract in place.  
Acceptance signal: template compile/validation succeeds via `vizier-kernel/src/workflow_template.rs` rules with no legacy `vizier.*` labels.

3. Harden queue-time rejection semantics for stage runs.  
Verify `vizier-cli/src/workflow_templates.rs` + `vizier run` enforce all-or-nothing queue-time failure for unresolved placeholders, invalid typed coercions, and legacy `uses` labels before manifest/job creation.  
Dependency: step 2 templates finalized.  
Acceptance signal: negative runs do not enqueue node jobs and do not persist run manifests.

4. Validate runtime execution exclusively through scheduler primitives.  
Confirm stage nodes materialize to scheduler jobs and execute via `vizier-cli/src/jobs.rs` canonical handlers, including worktree lifecycle, prompt/invoke path, plan/git integration, and gate outcomes. Validate operator control paths via `vizier jobs show|tail|attach|approve|retry|cancel|gc`.  
Dependency: steps 2–3 complete.  
Acceptance signal: stage runs can be observed and controlled end-to-end without command-specific runtime wrappers.

5. Update operator/dev documentation and spec alignment points.  
Refresh `docs/user/workflows/alias-run-flow.md`, `docs/user/workflows/stage-execution.md`, `docs/dev/scheduler-dag.md`, and `docs/dev/vizier-material-model.md` so they describe stage execution as template DAGs under `run`/`jobs` only, with canonical primitive identities and gate wiring expectations.  
Dependency: runtime behavior validated in step 4.  
Acceptance signal: docs no longer imply deprecated stage commands or wrapper-era runtime behavior.

6. Complete branch validation gates.  
Run `cargo check --all --all-targets`, `cargo test --all --all-targets`, and `./cicd.sh` after test/doc updates.  
Acceptance signal: all gates pass with stage-template coverage included.

## Risks & Unknowns
1. Snapshot indicates most canonical runtime plumbing is already landed; risk is duplicative churn versus true gap-closing. Mitigation: start with a strict gap audit against current `.vizier/workflow/*.toml` and existing `tests/src/run.rs`.
2. Approve retry-loop wiring can regress into non-terminating or misrouted flows if stop-gate cardinality/routing drifts. Mitigation: explicit loop-behavior integration tests with bounded retry assertions.
3. Merge conflict/CI/CD gate combinations can create routing ambiguity (especially with auto-resolve/retry). Mitigation: validate canonical edge wiring and blocked/failure transitions with deterministic job-state assertions.
4. Legacy branch/plan-doc drift in worktrees can confuse plan artifact expectations. Mitigation: keep runtime acceptance tied to emitted artifacts/metadata (`plan_branch`, `plan_doc`, job metadata), not branch/doc inventory bijection.

## Testing & Verification
1. Add/extend integration coverage in `tests/src/run.rs` for `vizier run draft`, `vizier run approve`, and `vizier run merge` stage-template smoke execution.
2. Add queue-time negative tests for legacy `uses = "vizier.*"` and unresolved/invalid `--set` coercions, asserting no partial enqueue/manifests.
3. Add approve-stage retry-loop tests that assert `stage_commit -> stop_gate -> stage_commit` closure and terminal behavior when retries exhaust or gate passes.
4. Add merge-stage tests for `git.integrate_plan_branch` plus optional `control.gate.conflict_resolution` and `control.gate.cicd` routing.
5. Validate `vizier jobs approve|retry|cancel|tail|attach` behavior against stage-run job records.
6. Keep removed-wrapper regression checks (unknown-subcommand path) and hidden `__workflow-node` visibility contract intact.
7. Run full branch gates: `cargo check --all --all-targets`, `cargo test --all --all-targets`, `./cicd.sh`.

## Notes
Current snapshot already reports canonical validator/runtime coverage and wrapper removal; implementation should therefore be a targeted conformance/hardening pass (aliases, stage-template contracts, tests, docs), not a new orchestration redesign.
