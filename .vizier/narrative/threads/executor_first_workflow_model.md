# Executor-first workflow model

Thread: Executor-first workflow taxonomy (cross: Jobs/read-only scheduler operations, Reduced CLI surface stabilization)

Snapshot anchor
- Active thread — Executor-first workflow taxonomy (Running Snapshot — updated).

Tension
- Legacy workflow capability IDs mixed executor behavior and control policy semantics, which made template validation ambiguous and harder to audit.
- Arbitrary non-`vizier.*` labels previously classified as executable custom capability via implicit fallback, which blurred safety boundaries.
- Jobs metadata exposed only legacy capability IDs, so executor identity was not directly visible in `vizier jobs show`.

Desired behavior (Product-level)
- Internal template validation classifies every node as either executor (`environment.builtin`, `environment.shell`, `agent`) or control.
- Executor nodes must declare explicit executor IDs; control nodes must declare control policies and never masquerade as executor operations.
- Workflow identity is canonical-only: accepted `uses` IDs are `cap.env.*`, `cap.agent.invoke`, and `control.*`.
- Unknown arbitrary `uses` labels are rejected; there is no implicit custom-command fallback.
- Jobs metadata/rendering shows executor identity (`workflow_executor_class`, `workflow_executor_operation`, optional `workflow_control_policy`) as the forward identity contract.

Acceptance criteria
- Validator rejects unknown implicit `uses` labels and kind/class mismatches with deterministic errors.
- Validator coverage confirms legacy `vizier.*` and legacy non-env `cap.*` labels are hard-rejected.
- Maintained `.vizier/workflow/*.toml` artifacts express env/agent/control boundaries explicitly without reintroducing removed top-level workflow commands.
- `vizier jobs show` surfaces executor identity fields for new records while remaining tolerant of historical records that still include legacy capability metadata.

Status
- Update (2026-02-14, canonical runtime completion):
  - `vizier-core/src/jobs/mod.rs` now executes all canonical runtime handlers accepted by `vizier-kernel` (`prompt.resolve`, `agent.invoke`, `worktree.prepare`, `worktree.cleanup`, `plan.persist`, `git.stage_commit`, `git.integrate_plan_branch`, `git.save_worktree_patch`, `patch.pipeline_prepare`, `patch.execute_pipeline`, `patch.pipeline_finalize`, `build.materialize_step`, `merge.sentinel.write`, `merge.sentinel.clear`, `command.run`, `cicd.run`, plus `gate.stop_condition`, `gate.conflict_resolution`, `gate.cicd`, `gate.approval`, and `terminal`).
  - `agent.invoke` now resolves real configured agent settings/runner and records backend metadata instead of using payload-echo facade behavior.
  - Runtime handlers now use execution-root resolution (repo root or job-linked worktree), enforce worktree ownership-safe cleanup semantics, and materialize plan/patch/build/sentinel artifacts with concrete failure/blocked outcomes.
  - Runtime coverage in `vizier-core/src/jobs/mod.rs` now includes operation-level success/failure tests across canonical executor/control inventory, and branch validation (`cargo check --all --all-targets`, `cargo test --all --all-targets`, `./cicd.sh`) is green.
  - Docs were updated to reflect implemented runtime semantics (`RUNTIME.md`, `docs/dev/scheduler-dag.md`, `docs/dev/vizier-material-model.md`, `docs/dev/testing.md`).
- Update (2026-02-14, runtime bridge):
  - `vizier-core/src/jobs/mod.rs` now exposes `enqueue_workflow_run` to compile canonical templates into one scheduler job per node with deterministic job IDs, canonical workflow metadata, and hidden `__workflow-node --job-id <id>` child args.
  - `vizier-cli/src/cli/args.rs` + `vizier-cli/src/cli/dispatch.rs` now wire the internal hidden `__workflow-node` entrypoint for scheduler child execution while keeping public help surfaces unchanged.
  - Runtime execution now records per-node metadata (`workflow_run_id`, `workflow_node_attempt`, `workflow_node_outcome`, `workflow_payload_refs`), writes run manifests under `.vizier/jobs/runs/<run_id>.json`, and persists prompt payload JSON under `.vizier/jobs/artifacts/data/...` while keeping marker files as scheduler truth.
  - Runtime routing now maps `on.succeeded` to queue-time `after` dependencies (single-parent constraint) and handles non-success routes via retry-driven target requeue.
  - Coverage landed in `vizier-core/src/jobs/mod.rs` for queue-time materialization, prompt payload roundtrip, stop-condition retry-budget blocking, and retry cleanup of marker+payload artifacts.
- Update (2026-02-14, canonical uses-only hard cut):
  - `vizier-kernel/src/workflow_template.rs` removed legacy alias translation (`vizier.*`, legacy non-env `cap.*`, and alias-window diagnostics), requires explicit non-empty canonical `uses` IDs, and validates workflow semantics by executor operation/control policy.
  - Canonical acceptance is now strict: only `cap.env.*`, `cap.agent.invoke`, and `control.*` compile.
  - `.vizier/workflow/{draft,approve,merge}.toml` now use canonical built-in IDs (`plan.persist`, `git.stage_commit`, `git.integrate_plan_branch`) with no legacy labels.
  - `vizier-cli` jobs metadata now treats executor identity fields as canonical; legacy `workflow_capability_id` is deserialize-only historical compatibility and no longer part of active display/merge posture.
  - Docs/tests were updated to remove compatibility-window language and assert hard rejection behavior.
- Update (2026-02-14): Landed v1 of executor/control split scaffolding.
  - `vizier-kernel/src/workflow_template.rs` now classifies nodes into executor/control identity, adds explicit executor/control metadata to compiled nodes, emits legacy alias diagnostics, and rejects unknown implicit `uses` labels.
  - Compatibility policy is documented with a hard-rejection date after `2026-06-01`.
  - `.vizier/workflow/{draft,approve,merge}.toml` and `.vizier/develop.toml` moved to v2 executor-first node chains.
  - `vizier-core/src/jobs/mod.rs`, `vizier-cli/src/cli/args.rs`, and `vizier-cli/src/cli/jobs_view.rs` now support/display executor identity metadata alongside legacy capability fields.
  - Docs updated: `docs/dev/scheduler-dag.md`, `docs/dev/vizier-material-model.md`, `docs/dev/testing.md`.
- Update (2026-02-14, invoke migration): Canonicalized agent execution to `cap.agent.invoke` with explicit prompt artifacts.
  - `vizier-kernel/src/workflow_template.rs` now maps canonical agent runtime execution to `agent.invoke`, keeps legacy purpose-specific agent IDs as warning aliases through `2026-06-01`, and adds validator contracts for canonical `prompt.resolve`/`agent.invoke` wiring.
  - Canonical validation now requires `custom:prompt_text:<key>` producer/consumer shape: prompt-resolve nodes emit exactly one prompt artifact and canonical invoke nodes consume exactly one prompt artifact with no inline command/script args.
  - `.vizier/workflow/{draft,approve,merge}.toml` now model explicit `prompt.resolve -> agent.invoke` chains with `prompt_text` artifact contracts (`v2` templates).
  - Runtime/observability coverage expanded: `vizier-core/src/agent.rs` now tests prompt stdin forwarding explicitly, and `vizier-cli/src/cli/jobs_view.rs` tests canonical `agent.invoke` metadata rendering alongside legacy capability fields.

Pointers
- `vizier-kernel/src/workflow_template.rs`
- `.vizier/workflow/draft.toml`
- `.vizier/workflow/approve.toml`
- `.vizier/workflow/merge.toml`
- `.vizier/develop.toml`
- `vizier-core/src/jobs/mod.rs`
- `vizier-cli/src/jobs.rs` (compatibility shim)
- `vizier-cli/src/cli/jobs_view.rs`
