# Workflow Runtime Contract

This document defines the internal workflow runtime behavior used by scheduler
child jobs (`vizier __workflow-node --job-id <id>`). It is intentionally
internal: it does not change the reduced public CLI surface.

## Scope

- Runtime dispatch for canonical executor operations and control policies
  compiled from workflow templates.
- Shared execution context, outcome normalization, and artifact/payload
  persistence.
- Behavior for scheduler-driven retries and gate routing.

## Execution Context

- Every node executes against a resolved execution root:
  - default: repository root
  - worktree-aware: `metadata.worktree_path` when present and valid
- Node results normalize to:
  - `outcome` (`succeeded` | `failed` | `blocked` | `cancelled`)
  - `artifacts_written[]`
  - `payload_refs[]`
  - optional `summary`
  - optional `exit_code`
  - optional metadata delta merged into the job record on finalize

## Canonical Runtime Operations

All canonical operations accepted by `vizier-kernel` validation are executable
at runtime.

### Executor operations

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

### Control policies

1. `gate.stop_condition`
2. `gate.conflict_resolution`
3. `gate.cicd`
4. `gate.approval`
5. `terminal`

Unknown runtime operation/policy identifiers continue to fail fast.

## Key Semantics

- `agent.invoke` consumes a prompt artifact payload and executes via resolved
  configured agent settings and runner (no prompt-echo facade path).
- Worktree lifecycle is ownership-checked:
  - `worktree.prepare` creates/records job-owned paths under
    `.vizier/tmp-worktrees/`
  - `worktree.cleanup` removes only owned/safe paths and records degraded
    cleanup metadata when cleanup cannot complete
- `plan.persist` writes:
  - `.vizier/implementation-plans/<slug>.md`
  - `.vizier/state/plans/<plan_id>.json`
  and emits `plan_branch` + `plan_doc` artifacts.
- `git.integrate_plan_branch` supports merge/squash integration and conflict
  blocking via merge sentinel material.
- Patch pipeline operations enforce `files_json` and materialize deterministic
  manifest/finalize files under `.vizier/jobs/<job_id>/`.
- `merge.sentinel.write` / `merge.sentinel.clear` deterministically manage
  `.vizier/tmp/merge-conflicts/<slug>.json`.
- Shell-backed runtime operations (`command.run`, `cicd.run`, gate scripts) run
  in execution root and return captured status semantics.
- `terminal` is an explicit sink policy: outgoing routes are treated as an
  invalid configuration.

## Runtime Metadata and Payloads

- Workflow node execution records:
  - `workflow_run_id`
  - `workflow_node_attempt`
  - `workflow_node_outcome`
  - `workflow_payload_refs`
- Prompt/custom artifacts keep marker files under
  `.vizier/jobs/artifacts/custom/...` as scheduler truth and may persist typed
  payload JSON under `.vizier/jobs/artifacts/data/...`.

## Verification

Current branch validation gates:

- `cargo check --all --all-targets`
- `cargo test --all --all-targets`
- `./cicd.sh`

Runtime unit coverage in `vizier-cli/src/jobs.rs` includes queue-time
materialization, prompt payload roundtrip, stop-condition retry behavior, and
success/failure coverage for each canonical executor operation and control
policy.
