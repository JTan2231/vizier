---
plan_id: pln_08693b0be1624e158f873e6cfe4d8168
plan: impl
branch: draft/impl
---

## Operator Spec
# Workflow Ops Completion Spec

## Status

- Proposed.
- Baseline date: 2026-02-14.
- Scope: complete runtime implementation for all canonical workflow operations/policies.

## Problem

The canonical template validator accepts a broad operation surface, but runtime dispatch currently only implements a subset.

Current runtime outcomes:

1. Implemented (real behavior):
- `prompt.resolve`
- `git.stage_commit`
- `gate.stop_condition` (with optional script-skip path)

2. Facade/stub behavior:
- `agent.invoke` (consumes prompt payload and prints it; no real backend invocation)
- `worktree.prepare` (success-only placeholder)
- `worktree.cleanup` (success-only placeholder)
- `control.terminal` (success-only placeholder)

3. Unsupported at runtime (hard-fail if routed):
- `plan.persist`
- `git.integrate_plan_branch`
- `git.save_worktree_patch`
- `patch.pipeline_prepare`
- `patch.pipeline_finalize`
- `patch.execute_pipeline`
- `build.materialize_step`
- `merge.sentinel.write`
- `merge.sentinel.clear`
- `command.run`
- `cicd.run`
- `gate.conflict_resolution`
- `gate.cicd`
- `gate.approval`

This prevents true reconstruction of deprecated draft/approve/merge behavior through canonical templates alone.

## Goals

1. Implement all 21 canonical runtime operations/policies with concrete behavior.
2. Replace stub/facade behaviors with production semantics.
3. Keep reduced public CLI surface unchanged (`init`, `list`, `jobs`, `release`, `completions`).
4. Preserve scheduler contracts (dependency/approval/locks/preconditions/retry/status).
5. Make canonical template composition sufficient to recreate draft/approve/merge flows.

## Non-Goals

1. Reintroduce deprecated top-level commands.
2. Re-enable legacy `vizier.*` or non-canonical `cap.*` labels.
3. Change scheduler DAG readiness ordering.

## Canonical Operation Inventory

Executor operations:

1. `prompt.resolve`
2. `agent.invoke`
3. `worktree.prepare`
4. `worktree.cleanup`
5. `plan.persist`
6. `git.stage_commit`
7. `git.integrate_plan_branch`
8. `git.save_worktree_patch`
9. `patch.pipeline_prepare`
10. `patch.pipeline_finalize`
11. `patch.execute_pipeline`
12. `build.materialize_step`
13. `merge.sentinel.write`
14. `merge.sentinel.clear`
15. `command.run`
16. `cicd.run`

Control policies:

1. `gate.stop_condition`
2. `gate.conflict_resolution`
3. `gate.cicd`
4. `gate.approval`
5. `terminal`

## Runtime Contract

## Common Execution Context

1. All executor/control handlers run against a resolved execution root:
- default: repo root
- overridden by job metadata worktree context when present
2. All handlers emit a normalized node result:
- `outcome`
- `artifacts_written`
- `payload_refs`
- `summary`
- `exit_code`
3. Handlers are idempotent for retries on the same job where feasible.

## Agent Invocation Contract (`agent.invoke`)

1. Replace payload-echo facade with real backend invocation via existing agent runner path.
2. Prompt source remains custom prompt artifact payload dependency (`custom:prompt_text:<key>`).
3. Resolve agent settings from config snapshot and workflow scope/template metadata.
4. Stream progress through existing progress/event path.
5. Capture and persist:
- backend/label/command metadata
- exit code
- payload refs/session refs as applicable
6. Non-zero backend exit marks node `failed`.

## Worktree Lifecycle (`worktree.prepare`, `worktree.cleanup`)

`worktree.prepare`:

1. Create isolated temp worktree under `.vizier/tmp-worktrees/`.
2. Bind to target branch (explicit arg or metadata-derived branch).
3. Record ownership and path metadata on the job.
4. Return `failed` when creation/setup fails.

`worktree.cleanup`:

1. Remove only worktrees marked as job-owned.
2. Prune git worktree metadata and filesystem directory.
3. Surface degraded cleanup details in metadata and warning logs.
4. Never delete non-owned paths.

## Plan Persistence (`plan.persist`)

1. Validate and resolve `spec_text` / `spec_file` / `spec_source`.
2. Generate or persist plan document in canonical location:
- `.vizier/implementation-plans/<slug>.md`
3. Persist/update plan state:
- `.vizier/state/plans/<plan_id>.json`
4. Write on target branch/worktree context and produce contract artifacts:
- `plan_branch`
- `plan_doc`

## Git Operations

`git.stage_commit`:

1. Keep existing add/diff/commit flow.
2. Run in execution root (worktree-aware).
3. Support explicit commit message args and no-change semantics.

`git.integrate_plan_branch`:

1. Implement merge/squash integration behavior using current merge primitives.
2. Respect retry/conflict routing contracts and outcome edges.
3. Write/consume merge sentinel metadata as needed.

`git.save_worktree_patch`:

1. Create patch artifact from worktree/index state.
2. Persist canonical patch file and produce artifact contract.

## Patch/Build/Merge Sentinel Operations

`patch.pipeline_prepare`, `patch.pipeline_finalize`, `patch.execute_pipeline`:

1. Implement file-based patch pipeline orchestration.
2. Enforce `files_json` and patch ordering contracts.

`build.materialize_step`:

1. Materialize build step state/metadata/artifacts for workflow chaining.

`merge.sentinel.write`, `merge.sentinel.clear`:

1. Write/clear `.vizier/tmp/merge-conflicts/<slug>.json` deterministically.
2. Preserve compatibility with retry/cancel cleanup behavior.

## Shell Operations

`command.run`:

1. Execute declared command/script in execution root.
2. Capture stdout/stderr, exit code, summary.
3. No implicit command fallback.

`cicd.run`:

1. Execute CI/CD script command with same capture semantics as `command.run`.
2. Integrate with cicd gate control loops.

## Control Policies

`gate.stop_condition`:

1. Keep script execution semantics.
2. Preserve retry-budget behavior.
3. Continue route-driven retry (`on.failed` target).

`gate.conflict_resolution`:

1. Resolve/validate conflict state using sentinel and git status.
2. Apply auto-resolve path when configured.
3. Route success/failure per template edges.

`gate.cicd`:

1. Run configured CI/CD gate script.
2. Support retry/auto-resolve loop closures.
3. Emit attempt telemetry.

`gate.approval`:

1. Enforce explicit approval decision from schedule metadata.
2. If pending/rejected, return blocked/failed outcome with reason.
3. Preserve compatibility with scheduler-level approval gate behavior.

`terminal`:

1. Keep as explicit workflow sink policy.
2. Validate that no invalid outgoing routes are configured.
3. Emit terminal summary metadata (not a blind placeholder).

## Cross-Cutting Requirements

1. Remove any remaining runtime operation fallthrough for canonical ops.
2. Unknown operations/policies remain hard failures.
3. Preserve artifact marker + payload store contracts.
4. Preserve lock/precondition/approval ordering in scheduler.
5. Maintain retry cleanup safety for worktree/artifact payloads.

## Rollout Plan

## Phase 1: Runtime Scaffolding

1. Split executor/control dispatch into per-operation handler modules.
2. Add shared execution-context resolver (repo root vs worktree root).
3. Add per-op telemetry helpers and error normalization.

## Phase 2: Replace Facades/Stubs

1. Implement real `agent.invoke`.
2. Implement real `worktree.prepare`.
3. Implement real `worktree.cleanup`.
4. Upgrade `terminal` from placeholder to explicit sink policy contract.

## Phase 3: Fill Unsupported Ops

1. Implement `plan.persist`.
2. Implement git integration/sentinel/patch/build/shell ops.
3. Implement `gate.conflict_resolution`, `gate.cicd`, `gate.approval`.

## Phase 4: Template and Flow Validation

1. Validate draft/approve/merge canonical template chains execute end-to-end.
2. Validate retry loops and gate behavior through scheduler jobs.

## Test Plan

Unit coverage:

1. One test per canonical operation for success/failure paths.
2. Contract tests for required args/artifacts for each op.
3. Metadata/payload persistence assertions per op.

Integration coverage:

1. Draft-style chain:
- `prompt.resolve -> agent.invoke -> plan.persist`
2. Approve-style chain:
- `worktree.prepare -> prompt.resolve -> agent.invoke -> git.stage_commit -> gate.stop_condition -> worktree.cleanup -> terminal`
3. Merge-style chain:
- `git.integrate_plan_branch -> gate.conflict_resolution/gate.cicd -> merge.sentinel.clear`
4. Patch/build chains for pipeline and materialization operations.

Scheduler/regression coverage:

1. No drift in wait/approval/lock/precondition behavior.
2. Retry/cancel cleanup still removes stale markers/payloads/worktrees safely.
3. Jobs views show correct workflow executor/control metadata for all ops.

## Acceptance Criteria

1. All 21 canonical operations/policies are executable (none are placeholder-only or unsupported).
2. `agent.invoke` performs real backend invocation through configured runner.
3. Worktree prepare/cleanup create and remove real isolated environments.
4. Draft/approve/merge behaviors are reproducible by canonical template composition without deprecated command wrappers.
5. End-to-end tests for canonical flow chains pass.
6. `./cicd.sh` passes after implementation and test updates.

## Documentation Updates Required

1. `RUNTIME.md` updated from proposed state to implemented state.
2. `docs/dev/scheduler-dag.md` updated with final operation behavior and sink/gate semantics.
3. `docs/dev/vizier-material-model.md` updated for new artifact/worktree/runtime invariants.
4. User docs updated only for observable behavior changes on retained commands.

## Implementation Plan
## Overview
This work completes Vizier’s internal workflow runtime so every canonical executor operation and control policy accepted by the validator is actually executable at runtime. Today, only a subset has real behavior, several are stubs, and many hard-fail, which blocks full canonical template composition for draft/approve/merge-style flows through scheduler jobs. The change primarily impacts maintainers/operators of workflow jobs (`vizier jobs` + hidden `__workflow-node`) and is needed now to align runtime behavior with the snapshot’s executor-first contract while keeping the reduced public CLI surface unchanged.

## Execution Plan
1. Lock the runtime contract against the snapshot baseline by enumerating all 21 canonical operations/policies and mapping each to a concrete handler path in the runtime dispatch layer (`vizier-cli/src/jobs.rs` and related runtime modules). Acceptance signal: no canonical op/policy remains in stub or unsupported fallthrough state.
2. Introduce shared runtime scaffolding for all handlers: execution-root resolution (repo root vs job-linked worktree), normalized node result shape (`outcome`, artifacts, payload refs, summary, exit code), and common telemetry/error normalization. Acceptance signal: all handlers emit the same result schema and worktree-aware execution root behavior.
3. Replace current facades/stubs with production semantics for `agent.invoke`, `worktree.prepare`, `worktree.cleanup`, and `terminal`. Acceptance signal: `agent.invoke` uses the real configured runner path, worktree lifecycle creates/removes owned temp worktrees safely, and `terminal` enforces explicit sink-policy semantics instead of placeholder success.
4. Implement missing executor operations in dependency order: `plan.persist`; git operations (`git.integrate_plan_branch`, `git.save_worktree_patch`); patch/build/sentinel operations (`patch.*`, `build.materialize_step`, `merge.sentinel.*`); shell/gate execution primitives (`command.run`, `cicd.run`). Acceptance signal: each operation has deterministic success/failure behavior, required-arg validation, and explicit artifact/payload outputs.
5. Implement missing control policies: `gate.conflict_resolution`, `gate.cicd`, and `gate.approval`, preserving scheduler contracts for retries, approval state, routing, and lock/precondition ordering. Acceptance signal: policy outcomes route correctly (`succeeded`/`failed`/`blocked`) with attempt telemetry and no scheduler ordering regressions.
6. Validate canonical flow composition end-to-end using maintained workflow templates and hidden node execution (`__workflow-node`) without reintroducing removed top-level commands. Acceptance signal: draft/approve/merge-equivalent chains run fully through canonical template nodes and scheduler jobs only.
7. Align docs with implemented runtime behavior: update `RUNTIME.md`, `docs/dev/scheduler-dag.md`, and `docs/dev/vizier-material-model.md`; only touch user docs where retained command behavior is observably changed. Acceptance signal: docs describe final runtime semantics and artifact/worktree invariants with no references to removed command surfaces.

## Risks & Unknowns
- The operator spec lists retained commands as `init/list/jobs/release/completions`, while the snapshot’s canonical retained set also includes `help`, `cd`, and `clean`; implementation must preserve the full snapshot-defined surface and treat the spec list as non-exhaustive.
- `agent.invoke` moving from facade to real runner may expose backend/config edge cases (missing binaries, unsupported settings, noisy telemetry) that did not matter when it only echoed payloads.
- Worktree cleanup is safety-critical; ownership checks must be strict to avoid deleting non-owned paths, especially under retry/cancel paths.
- Recreating merge/patch/build behavior from canonical operations may surface gaps where wrapper-era assumptions were not yet encoded as explicit node args/artifacts.
- `gate.approval` semantics must stay compatible with scheduler-level approval behavior to avoid double-gating ambiguity.

## Testing & Verification
1. Add operation-level unit coverage for every canonical executor operation and control policy, including success/failure and required-argument/artifact contract validation.
2. Extend runtime integration coverage for canonical chains: `prompt.resolve -> agent.invoke -> plan.persist`; approve-style worktree/commit/stop-condition flow; merge-style integrate/conflict-cicd/sentinel-clear flow; patch/build pipeline flows.
3. Add scheduler regression checks for dependency readiness, approval/lock/precondition ordering, retry/cancel cleanup safety, and rewind cleanup of payload refs/artifacts/worktree state.
4. Preserve CLI-surface invariants with help/negative tests: removed commands still fail via Clap unknown-subcommand behavior, `__workflow-node` remains hidden from help/man/completions, retained commands/help pages stay unchanged.
5. Verify jobs observability for canonical metadata fields (`workflow_executor_class`, `workflow_executor_operation`, `workflow_control_policy`, node runtime metadata) while remaining tolerant of legacy persisted records.
6. Run full validation gates required by repo discipline: `cargo check --all --all-targets`, `cargo test --all --all-targets`, and `./cicd.sh`.

## Notes
- This plan is intentionally runtime-internal: it restores canonical operation completeness without reopening removed public workflow/agent command families.
- Legacy branch/plan-doc drift remains an active archival-hygiene thread; `plan.persist` implementation should avoid worsening drift while preserving compatibility with historical artifacts.
