---
plan_id: pln_c04b889addaf4683bf25bb8f02714181
plan: jobs
branch: draft/jobs
---

## Operator Spec
# Feature Spec: Scheduler/Jobs Refactor (`JOBS`)

## Status

- Proposed implementation spec.
- Scope date: 2026-02-15.
- This spec defines the target ownership and migration plan for `vizier-cli/src/jobs.rs`.

## Purpose

`vizier-cli/src/jobs.rs` is currently oversized and mixes concerns:

- CLI-facing UX concerns.
- Driver/runtime side effects.
- Scheduler orchestration and persistence.
- Workflow runtime bridge execution.

The current file is hard to maintain, difficult to reason about, and blurs crate boundaries defined by architecture docs.

Goals:

1. Restore clear crate boundaries (`kernel` pure, `core` side effects, `cli` UX).
2. Split jobs/scheduler/runtime code into focused modules.
3. Preserve existing behavior and command contract during migration.
4. Make future scheduler/runtime changes low-risk and testable.

## Decision

Scheduler/job/workflow runtime side-effect logic MUST live in `vizier-core`.

`vizier-cli` MUST remain a thin frontend:

- arg parsing
- command dispatch
- help/completions
- jobs/run output formatting
- watch/tail presentation UX

`vizier-kernel` remains pure and continues owning scheduler semantics (`spec`).

## Scope

In scope:

- Moving non-UX scheduler/job/workflow logic out of `vizier-cli/src/jobs.rs`.
- Introducing a modular `vizier-core/src/jobs/` package.
- Keeping command surface stable (`vizier jobs ...`, `vizier run`, `vizier __workflow-node`).
- Updating docs and tests to match new ownership.

Out of scope:

- Changing CLI command names or flags.
- Changing scheduler semantics/rules in `vizier-kernel`.
- Reworking template semantics or executor taxonomy.

## Ownership Boundaries

### `vizier-kernel` (unchanged)

- Pure scheduler decision logic.
- No filesystem/git/process/tty side effects.

Primary file:

- `vizier-kernel/src/scheduler/spec.rs`

### `vizier-core` (target owner)

- Job record models + persistence.
- Scheduler locking + tick orchestration.
- Fact extraction from git/filesystem/job store.
- Workflow run enqueue/materialization.
- Workflow node runtime execution bridge.
- Retry/approve/reject/cancel/gc mutations.
- Log tailing primitives.
- Worktree cleanup/retry cleanup internals.

### `vizier-cli` (target owner)

- `clap` argument definitions and parsing.
- Command dispatch wiring.
- Jobs table/json/block rendering.
- Jobs watch mode frame rendering.
- Follow-mode user summaries for `vizier run`.

## Exact Module Placement

Create `vizier-core/src/jobs/` with:

1. `model.rs`
   - `JobRecord`, `JobSchedule`, `JobMetadata`, `JobOutcome`
   - `JobApproval`, `WorkflowNodeResult`, snapshot/graph DTOs
2. `store.rs`
   - `ensure_jobs_root`, paths, read/write/update/list helpers
3. `lock.rs`
   - scheduler lock file acquisition/release
4. `lifecycle.rs`
   - `enqueue_job`, `start_job`, `finalize_job`, outcome writes
5. `graph.rs`
   - `ScheduleGraph`, edge derivation, snapshot helpers
6. `facts.rs`
   - scheduler fact extraction (`build_scheduler_facts`) and probes
7. `scheduler.rs`
   - `scheduler_tick`, `scheduler_tick_locked`
8. `workflow_enqueue.rs`
   - `enqueue_workflow_run`, manifest read/write, compile metadata
9. `workflow_runtime.rs`
   - `run_workflow_node_command`, node executor/control dispatch, route application
10. `ops.rs`
    - `approve_job`, `reject_job`, `retry_job`, `cancel_job_with_cleanup`, `gc_jobs`
11. `logs.rs`
    - `tail_job_logs`, `latest_job_log_line`, raw follow helpers
12. `cleanup.rs`
    - worktree cleanup and retry/cancel cleanup support

Add public facade:

- `vizier-core/src/jobs/mod.rs` re-exporting stable API consumed by CLI.

Keep in `vizier-cli`:

- `vizier-cli/src/cli/jobs_view.rs`
- `vizier-cli/src/cli/dispatch.rs`
- `vizier-cli/src/actions/run.rs` (input mapping + UX summaries)

Temporary compatibility shim:

- Keep `vizier-cli/src/jobs.rs` as thin re-export/wrapper during migration.

## Plan Helper Dependency

Current jobs/runtime logic depends on `crate::plan::*` from `vizier-cli`.

To complete ownership cleanly:

1. Move reusable plan-domain helpers used by runtime into `vizier-core` (for example `vizier-core/src/plan.rs`).
2. Keep CLI-only plan presentation in `vizier-cli`.
3. Remove `vizier-core -> vizier-cli` dependency risk entirely.

## Migration Strategy

### Phase 1: Scaffolding

1. Add `vizier-core/src/jobs/mod.rs` and empty submodules.
2. Add a compatibility export surface matching current `crate::jobs` API.

### Phase 2: Data + Store

1. Move model types and serde contracts.
2. Move store/path helpers and lock handling.
3. Keep all tests green before moving runtime logic.

### Phase 3: Scheduler Orchestration

1. Move graph/facts/tick orchestration.
2. Keep `vizier-kernel` spec usage unchanged.

### Phase 4: Workflow Runtime

1. Move enqueue + manifest logic.
2. Move `__workflow-node` runtime executor/control dispatch.
3. Move cleanup and route propagation internals.

### Phase 5: Operator Ops + Logs

1. Move retry/approve/reject/cancel/gc.
2. Move log tail/follow primitives.

### Phase 6: CLI Thinning

1. Replace direct `vizier-cli::jobs` logic with `vizier_core::jobs` calls.
2. Reduce `vizier-cli/src/jobs.rs` to shim or remove entirely.

### Phase 7: Final Cleanup

1. Delete dead wrappers.
2. Update docs and test file references.

## Backward Compatibility Contract

The following MUST remain stable during migration:

1. CLI commands and flags.
2. `.vizier/jobs/*` on-disk formats (`job.json`, logs, `outcome.json`, manifests).
3. Scheduler status labels and wait-reason semantics.
4. Exit code behavior (`run --follow`, blocked vs failed cases).

Any format changes require a separate migration spec.

## Testing Requirements

Required updates:

1. Move unit tests currently in `vizier-cli/src/jobs.rs` into corresponding `vizier-core/src/jobs/*` modules.
2. Keep kernel scheduler-spec tests in `vizier-kernel/src/scheduler/spec.rs`.
3. Keep CLI rendering tests in `vizier-cli/src/cli/jobs_view.rs`.
4. Keep integration behavior tests under `tests/src/*`.
5. Add targeted tests for module boundaries:
   - core jobs API can be used without CLI modules
   - no reverse dependency from core to CLI

Validation:

1. Run `./cicd.sh` for code changes in this refactor.
2. Ensure no regression in `tests/src/background.rs`, `tests/src/jobs.rs`, and `tests/src/run.rs`.

## Documentation Updates

This refactor MUST update:

1. `docs/dev/code-organization.md`
2. `docs/dev/architecture/drivers.md`
3. `docs/dev/testing.md`
4. `docs/dev/scheduler-dag.md`

## Acceptance Criteria

1. `vizier-cli/src/jobs.rs` is either removed or reduced to a thin compatibility shim.
2. Side-effectful scheduler/job/runtime logic resides in `vizier-core/src/jobs/*`.
3. `vizier-kernel` remains pure; no new side effects added.
4. Public CLI behavior and on-disk job artifacts remain unchanged.
5. Tests pass and `./cicd.sh` passes after the refactor.
6. Documentation reflects the new ownership model and file layout.

## Risks and Mitigations

1. Risk: behavioral drift during large moves.
   - Mitigation: phase-by-phase extraction with no-op wrappers and frequent test runs.
2. Risk: hidden coupling to CLI `plan` helpers.
   - Mitigation: extract shared plan helpers to core early.
3. Risk: accidental JSON schema drift.
   - Mitigation: preserve type names/serde tags and add serialization regression tests.

## References

- `vizier-cli/src/jobs.rs`
- `vizier-cli/src/cli/jobs_view.rs`
- `vizier-kernel/src/scheduler/spec.rs`
- `docs/dev/architecture/kernel.md`
- `docs/dev/architecture/drivers.md`
- `docs/dev/architecture/ports.md`
- `docs/dev/testing.md`

## Implementation Plan
# Feature Spec: Scheduler/Jobs Refactor (`JOBS`)

## Status

- Proposed implementation spec.
- Scope date: 2026-02-15.
- This spec defines the target ownership and migration plan for `vizier-cli/src/jobs.rs`.

## Purpose

`vizier-cli/src/jobs.rs` is currently oversized and mixes concerns:

- CLI-facing UX concerns.
- Driver/runtime side effects.
- Scheduler orchestration and persistence.
- Workflow runtime bridge execution.

The current file is hard to maintain, difficult to reason about, and blurs crate boundaries defined by architecture docs.

Goals:

1. Restore clear crate boundaries (`kernel` pure, `core` side effects, `cli` UX).
2. Split jobs/scheduler/runtime code into focused modules.
3. Preserve existing behavior and command contract during migration.
4. Make future scheduler/runtime changes low-risk and testable.

## Decision

Scheduler/job/workflow runtime side-effect logic MUST live in `vizier-core`.

`vizier-cli` MUST remain a thin frontend:

- arg parsing
- command dispatch
- help/completions
- jobs/run output formatting
- watch/tail presentation UX

`vizier-kernel` remains pure and continues owning scheduler semantics (`spec`).

## Scope

In scope:

- Moving non-UX scheduler/job/workflow logic out of `vizier-cli/src/jobs.rs`.
- Introducing a modular `vizier-core/src/jobs/` package.
- Keeping command surface stable (`vizier jobs ...`, `vizier run`, `vizier __workflow-node`).
- Updating docs and tests to match new ownership.

Out of scope:

- Changing CLI command names or flags.
- Changing scheduler semantics/rules in `vizier-kernel`.
- Reworking template semantics or executor taxonomy.

## Ownership Boundaries

### `vizier-kernel` (unchanged)

- Pure scheduler decision logic.
- No filesystem/git/process/tty side effects.

Primary file:

- `vizier-kernel/src/scheduler/spec.rs`

### `vizier-core` (target owner)

- Job record models + persistence.
- Scheduler locking + tick orchestration.
- Fact extraction from git/filesystem/job store.
- Workflow run enqueue/materialization.
- Workflow node runtime execution bridge.
- Retry/approve/reject/cancel/gc mutations.
- Log tailing primitives.
- Worktree cleanup/retry cleanup internals.

### `vizier-cli` (target owner)

- `clap` argument definitions and parsing.
- Command dispatch wiring.
- Jobs table/json/block rendering.
- Jobs watch mode frame rendering.
- Follow-mode user summaries for `vizier run`.

## Exact Module Placement

Create `vizier-core/src/jobs/` with:

1. `model.rs`
   - `JobRecord`, `JobSchedule`, `JobMetadata`, `JobOutcome`
   - `JobApproval`, `WorkflowNodeResult`, snapshot/graph DTOs
2. `store.rs`
   - `ensure_jobs_root`, paths, read/write/update/list helpers
3. `lock.rs`
   - scheduler lock file acquisition/release
4. `lifecycle.rs`
   - `enqueue_job`, `start_job`, `finalize_job`, outcome writes
5. `graph.rs`
   - `ScheduleGraph`, edge derivation, snapshot helpers
6. `facts.rs`
   - scheduler fact extraction (`build_scheduler_facts`) and probes
7. `scheduler.rs`
   - `scheduler_tick`, `scheduler_tick_locked`
8. `workflow_enqueue.rs`
   - `enqueue_workflow_run`, manifest read/write, compile metadata
9. `workflow_runtime.rs`
   - `run_workflow_node_command`, node executor/control dispatch, route application
10. `ops.rs`
    - `approve_job`, `reject_job`, `retry_job`, `cancel_job_with_cleanup`, `gc_jobs`
11. `logs.rs`
    - `tail_job_logs`, `latest_job_log_line`, raw follow helpers
12. `cleanup.rs`
    - worktree cleanup and retry/cancel cleanup support

Add public facade:

- `vizier-core/src/jobs/mod.rs` re-exporting stable API consumed by CLI.

Keep in `vizier-cli`:

- `vizier-cli/src/cli/jobs_view.rs`
- `vizier-cli/src/cli/dispatch.rs`
- `vizier-cli/src/actions/run.rs` (input mapping + UX summaries)

Temporary compatibility shim:

- Keep `vizier-cli/src/jobs.rs` as thin re-export/wrapper during migration.

## Plan Helper Dependency

Current jobs/runtime logic depends on `crate::plan::*` from `vizier-cli`.

To complete ownership cleanly:

1. Move reusable plan-domain helpers used by runtime into `vizier-core` (for example `vizier-core/src/plan.rs`).
2. Keep CLI-only plan presentation in `vizier-cli`.
3. Remove `vizier-core -> vizier-cli` dependency risk entirely.

## Migration Strategy

### Phase 1: Scaffolding

1. Add `vizier-core/src/jobs/mod.rs` and empty submodules.
2. Add a compatibility export surface matching current `crate::jobs` API.

### Phase 2: Data + Store

1. Move model types and serde contracts.
2. Move store/path helpers and lock handling.
3. Keep all tests green before moving runtime logic.

### Phase 3: Scheduler Orchestration

1. Move graph/facts/tick orchestration.
2. Keep `vizier-kernel` spec usage unchanged.

### Phase 4: Workflow Runtime

1. Move enqueue + manifest logic.
2. Move `__workflow-node` runtime executor/control dispatch.
3. Move cleanup and route propagation internals.

### Phase 5: Operator Ops + Logs

1. Move retry/approve/reject/cancel/gc.
2. Move log tail/follow primitives.

### Phase 6: CLI Thinning

1. Replace direct `vizier-cli::jobs` logic with `vizier_core::jobs` calls.
2. Reduce `vizier-cli/src/jobs.rs` to shim or remove entirely.

### Phase 7: Final Cleanup

1. Delete dead wrappers.
2. Update docs and test file references.

## Backward Compatibility Contract

The following MUST remain stable during migration:

1. CLI commands and flags.
2. `.vizier/jobs/*` on-disk formats (`job.json`, logs, `outcome.json`, manifests).
3. Scheduler status labels and wait-reason semantics.
4. Exit code behavior (`run --follow`, blocked vs failed cases).

Any format changes require a separate migration spec.

## Testing Requirements

Required updates:

1. Move unit tests currently in `vizier-cli/src/jobs.rs` into corresponding `vizier-core/src/jobs/*` modules.
2. Keep kernel scheduler-spec tests in `vizier-kernel/src/scheduler/spec.rs`.
3. Keep CLI rendering tests in `vizier-cli/src/cli/jobs_view.rs`.
4. Keep integration behavior tests under `tests/src/*`.
5. Add targeted tests for module boundaries:
   - core jobs API can be used without CLI modules
   - no reverse dependency from core to CLI

Validation:

1. Run `./cicd.sh` for code changes in this refactor.
2. Ensure no regression in `tests/src/background.rs`, `tests/src/jobs.rs`, and `tests/src/run.rs`.

## Documentation Updates

This refactor MUST update:

1. `docs/dev/code-organization.md`
2. `docs/dev/architecture/drivers.md`
3. `docs/dev/testing.md`
4. `docs/dev/scheduler-dag.md`

## Acceptance Criteria

1. `vizier-cli/src/jobs.rs` is either removed or reduced to a thin compatibility shim.
2. Side-effectful scheduler/job/runtime logic resides in `vizier-core/src/jobs/*`.
3. `vizier-kernel` remains pure; no new side effects added.
4. Public CLI behavior and on-disk job artifacts remain unchanged.
5. Tests pass and `./cicd.sh` passes after the refactor.
6. Documentation reflects the new ownership model and file layout.

## Risks and Mitigations

1. Risk: behavioral drift during large moves.
   - Mitigation: phase-by-phase extraction with no-op wrappers and frequent test runs.
2. Risk: hidden coupling to CLI `plan` helpers.
   - Mitigation: extract shared plan helpers to core early.
3. Risk: accidental JSON schema drift.
   - Mitigation: preserve type names/serde tags and add serialization regression tests.

## References

- `vizier-cli/src/jobs.rs`
- `vizier-cli/src/cli/jobs_view.rs`
- `vizier-kernel/src/scheduler/spec.rs`
- `docs/dev/architecture/kernel.md`
- `docs/dev/architecture/drivers.md`
- `docs/dev/architecture/ports.md`
- `docs/dev/testing.md`
