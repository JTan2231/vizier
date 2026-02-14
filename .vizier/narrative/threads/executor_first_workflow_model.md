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
- Legacy `cap.*`/legacy `vizier.*` labels remain temporarily compatible but emit warning diagnostics tied to an explicit deprecation window.
- Unknown arbitrary `uses` labels are rejected; there is no implicit custom-command fallback.
- Jobs metadata/rendering can show executor identity (`workflow_executor_class`, `workflow_executor_operation`, optional `workflow_control_policy`) while retaining legacy `workflow_capability_id` compatibility.

Acceptance criteria
- Validator rejects unknown implicit `uses` labels and kind/class mismatches with deterministic errors.
- Validator/diagnostic coverage confirms legacy alias warnings include the compatibility deadline (`2026-06-01`).
- Maintained `.vizier/workflow/*.toml` artifacts express env/agent/control boundaries explicitly without reintroducing removed top-level workflow commands.
- `vizier jobs show` can surface executor identity fields when present and still render legacy capability metadata for historical records.

Status
- Update (2026-02-14): Landed v1 of executor/control split scaffolding.
  - `vizier-kernel/src/workflow_template.rs` now classifies nodes into executor/control identity, adds explicit executor/control metadata to compiled nodes, emits legacy alias diagnostics, and rejects unknown implicit `uses` labels.
  - Compatibility policy is documented with a hard-rejection date after `2026-06-01`.
  - `.vizier/workflow/{draft,approve,merge}.toml` and `.vizier/develop.toml` moved to v2 executor-first node chains.
  - `vizier-cli/src/jobs.rs`, `vizier-cli/src/cli/args.rs`, and `vizier-cli/src/cli/jobs_view.rs` now support/display executor identity metadata alongside legacy capability fields.
  - Docs updated: `docs/dev/scheduler-dag.md`, `docs/dev/vizier-material-model.md`, `docs/dev/testing.md`.

Pointers
- `vizier-kernel/src/workflow_template.rs`
- `.vizier/workflow/draft.toml`
- `.vizier/workflow/approve.toml`
- `.vizier/workflow/merge.toml`
- `.vizier/develop.toml`
- `vizier-cli/src/jobs.rs`
- `vizier-cli/src/cli/jobs_view.rs`
