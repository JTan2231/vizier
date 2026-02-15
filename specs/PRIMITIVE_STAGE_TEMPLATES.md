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
