# Workflow-template reduction surface

Status (2026-02-14): ACTIVE. `vizier run` is now the public workflow-template orchestrator while wrapper families remain removed.

Thread: Workflow-template reduction surface (cross: Agent workflow orchestration, Configuration posture + defaults, Session logging)

Snapshot anchor
- Active threads — Workflow-template reduction surface (Running Snapshot — updated).
- Code state — Workflow-template contract/compiler path + template-config visibility + build resume policy snapshots.

Tension
- Scheduler primitives are generic, but command wrappers still hide orchestration details behind command-local loops and ad-hoc state checks.
- Operators and auditors need one durable contract that explains why a run waited/retried/blocked and whether resume is still safe after policy drift.
- Without a config-first template layer, extending workflows risks adding more command-specific branching and uneven observability.

Desired behavior (Product-level)
- Keep wrapper command removals intact while routing public orchestration through `vizier run` on top of the shared template contract (`Node`, `Edge`, `Template`, `PolicySnapshot`).
- Make template selection explicit and configurable per command scope via repo config, with resolved values visible in `vizier plan` and job/session artifacts.
- Persist policy snapshot identity for resumable workflows so compatibility checks can fail with actionable categories instead of opaque mismatch strings.
- Surface workflow metadata in jobs/sessions so operators can audit which template/node/gate policy drove a run without reading command internals.

Acceptance criteria
- Kernel exposes deterministic template + policy snapshot types with stable hashing and compile helpers for mapping to scheduler primitives.
- Config supports per-command template references, legacy configs remain valid, and resolved template refs appear in `vizier plan` text/JSON output.
- Scheduler/job artifacts include workflow template and policy snapshot metadata for command runs that compile templates.
- Build resume compatibility checks classify drift as node/edge/policy/artifact mismatch and preserve existing safety behavior.
- Integration coverage keeps wrapper behavior parity while asserting new metadata/reporting surfaces.

Status
- Update (2026-02-14, run front-door restoration):
  - `vizier-cli` now exposes `vizier run <flow>` with flow resolution (`file:`/path, `[commands]` alias, selector lookup, repo fallback files), composed-template support (`imports` + `links`), `${key}` parameter expansion via `--set`, root scheduling overrides (`--after`, approval toggles), and follow-mode terminal exit semantics.
  - Queue-time orchestration now calls `enqueue_workflow_run` from the new `run` action, preserving canonical runtime execution through hidden `__workflow-node`.
  - Integration coverage (`tests/src/run.rs`) now asserts alias/file execution, set-override expansion, legacy `uses` rejection without partial enqueue, root dependency/approval overrides, and follow exit mapping.
- Update (2026-02-09): Phase 1-3 scaffolding landed. `vizier-kernel/src/workflow_template.rs` now defines the canonical template/policy snapshot contract and hashing; `vizier-cli/src/workflow_templates.rs` resolves/compiles scoped template refs; config now exposes `[workflow.templates]` defaults and overrides; `vizier plan` reports resolved template mappings; jobs/build execution persist workflow template + policy snapshot metadata; resume mismatches now emit node/edge/policy/artifact diagnostics. Docs (`docs/user/build.md`, `docs/user/config-reference.md`, `docs/dev/scheduler-dag.md`, `docs/dev/vizier-material-model.md`) and tests were updated, and `./cicd.sh` is green.
- Update (2026-02-09, follow-up): `vizier build execute` phase scheduling now compiles `template.build_execute` nodes directly for `materialize`/`approve`/`review`/`merge`, so queued phase jobs inherit template locks, gate labels, retry policy, and explicit `schedule.after` edges from node relationships rather than hand-built phase structs. Build template gate config now derives from `[merge.cicd_gate]` (script/auto-resolve/retries), and build integration coverage asserts the compiled chaining behavior.
- Update (2026-02-13): Capability-boundary migration advanced: `vizier-kernel/src/workflow_template.rs` now enforces schedulable capability contracts at compile-time (approve/review/merge loop wiring, gate cardinality, custom-command fallback shape, and schedulable arg-shape checks), `compile_template_node_schedule` now runs that validator, wrapper/build schedulers precompile full node sets before enqueue to avoid partial graph creation on invalid templates, and review runtime now resolves primary review nodes semantically instead of requiring canonical `review_critique`.
- Update (2026-02-13, composed-alias run): `vizier-cli/src/workflow_templates.rs` now supports file-template composition via `imports` + `links` with deterministic prefixing, cycle detection, and link endpoint validation. `vizier run <alias>` now schedules repo-defined aliases through the same compiled-node DAG path used by built-ins, resolving `[commands.<alias>]` first and then repo fallback files (`.vizier/<alias>.toml|json`, `.vizier/workflow/<alias>.toml|json`). Integration coverage (`tests/src/run.rs`) now asserts composed stage ordering, selector precedence, fallback execution, and downstream blocking semantics.
- Remaining work: move remaining approve/review/merge command-local gate/retry loops behind compiled template nodes and align template/gate metadata with the unified Outcome/session schema once `outcome.v1` lands.

Pointers
- `vizier-kernel/src/workflow_template.rs`
- `vizier-kernel/src/config/{schema.rs,defaults.rs,merge.rs}`
- `vizier-core/src/config/load.rs`
- `vizier-cli/src/workflow_templates.rs`
- `vizier-cli/src/cli/dispatch.rs`
- `vizier-cli/src/actions/build.rs`
- `vizier-cli/src/actions/plan.rs`
