# Workflow-template reduction surface

Status (2026-02-15): ACTIVE. `vizier run` is now the public workflow-template orchestrator while wrapper families remain removed.

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
- Update (2026-02-19, HCL template cutover):
  - `vizier-cli/src/workflow_templates.rs` now parses `.hcl` workflow sources via vendored `rshcl` (`third_party/rshcl`), converts evaluated values to JSON/serde for `WorkflowTemplateFile`, reports path-anchored HCL diagnostics, and keeps legacy `.toml`/`.json` loading during migration.
  - Resolver identity scanning now includes `.hcl` candidates and applies deterministic same-stem precedence (`.hcl` wins over `.toml`), while explicit/global-flow resolution posture remains unchanged (no implicit alias fallback discovery).
  - Init/install/default assets moved to `.hcl`: stage/commit templates under `.vizier/workflows/*.hcl`, composed alias file `.vizier/develop.hcl`, `[commands]` defaults pointing to `file:.vizier/workflows/*.hcl`, and `install.sh` global seeding/manifests targeting `draft.hcl`, `approve.hcl`, and `merge.hcl`.
  - Docs/man/test surfaces were updated to HCL-first authoring guidance, including `$${key}` placeholder-escaping guidance for queue-time `${key}` interpolation boundaries.
- Update (2026-02-19, run help + preflight UX contract):
  - `vizier-cli/src/cli/dispatch.rs` now intercepts `vizier run <flow> --help` before generic Clap help short-circuiting, resolves flow/config with the same run path (alias/file/selector + config layering), and renders workflow-scoped help text.
  - `vizier-cli/src/cli/help.rs` now emits flow-scoped sections (`Workflow`, `Usage`, `Inputs`, `Examples`, `Run options`), including alias-to-param mappings and no-`[cli]` fallback guidance (`--set key=value`).
  - `vizier-cli/src/actions/run.rs` entry preflight now emits CLI-shaped missing-input guidance (`error`, `usage`, `example`, `hint`) and removes internal node/capability wording from primary user-facing text.
  - Coverage in `tests/src/help.rs` + `tests/src/run.rs` now asserts alias/file help parity, unchanged generic `vizier run --help`, no enqueue side effects for help paths, and the new missing-input error contract.
- Update (2026-02-18, run validate-only preflight):
  - `vizier-cli/src/cli/args.rs` adds run-local `--check` with explicit conflicts against enqueue/runtime flags (`--follow`, `--after`, `--require-approval`, `--no-require-approval`, `--repeat`), and run arg normalization now preserves `--check` instead of rewriting it into `--set`.
  - `vizier-core/src/jobs/mod.rs` now exposes shared pre-enqueue validation (`validate_workflow_run_template`) that reuses capability validation + full node compilation (resolved-after mapping plus single-parent succeeded-edge checks) for parity between validate-only and enqueue paths.
  - `vizier-cli/src/actions/run.rs` now branches after existing queue-time preprocessing (`resolve/load`, input mapping, entry preflight, stage `spec_file` inlining): `--check` emits validate-only output (`workflow_validation_passed`) and exits before run-id generation, manifest writes, job enqueue, and scheduler ticks.
  - Coverage in `tests/src/run.rs` now asserts check-mode success JSON shape, no side effects (no run manifests/jobs), unresolved/coercion/legacy-uses failure parity, and invalid flag-combination rejection.
  - Operator docs now include `vizier run --check` authoring guidance (`docs/user/workflows/alias-run-flow.md`, `docs/man/man7/vizier-workflow-template.7`, `docs/man/man7/vizier-workflow.7`).
- Update (2026-02-18, run-local repeat orchestration):
  - `vizier-cli/src/cli/args.rs` adds run-local `--repeat <N>` (`NonZeroU32`, default `1`), and run arg normalization in `vizier-cli/src/cli/util.rs` now preserves `--repeat N` / `--repeat=N` instead of rewriting to `--set`.
  - `vizier-cli/src/actions/run.rs` now supports repeat enqueue cycles with deterministic serial chaining (`i>1` appends `run:<prev_run_id>` sink dependencies), applies alias metadata + approval overrides for every iteration, and ticks scheduler once per iteration after root overrides persist.
  - Repeat-mode output contracts now emit aggregate summaries (`workflow_runs_enqueued` / `workflow_runs_terminal`, `repeat`, ordered per-run entries), while `repeat=1` preserves the single-run JSON shape for compatibility.
  - Repeat follow mode tracks runs in enqueue order and short-circuits on the first non-success terminal run (`blocked`/`failed`) with existing exit-code mapping.
  - Coverage additions include CLI parse/normalization checks and integration assertions for repeat enqueue chaining, `--after` composition, approval propagation, and follow short-circuit behavior (`tests/src/run.rs`).
- Update (2026-02-17, grouped run `--after` references):
  - `vizier-cli/src/actions/run.rs` now normalizes `--after` references into concrete job dependencies before root enqueue validation, accepting both direct `job_id` values and `run:<run_id>` tokens.
  - `run:<run_id>` expansion reads `.vizier/jobs/runs/<run_id>.json`, selects success-terminal sinks (`routes.succeeded` empty), rejects missing/unreadable manifests or zero-sink manifests, and rejects duplicate/empty sink `job_id` values with run-id-attributed errors.
  - Bare `run_<id>` references are now rejected with explicit guidance to use `run:<run_id>`.
  - Operator/docs surfaces now describe the expanded contract (`vizier-cli/src/cli/args.rs`, `docs/man/man7/vizier-workflow-template.7`, `docs/user/workflows/alias-run-flow.md`, `docs/dev/scheduler-dag.md`).
  - Integration coverage in `tests/src/run.rs` now asserts happy-path sink expansion, mixed run+job references, missing-manifest failure, zero-sink failure, and bare-id rejection guidance.
- Update (2026-02-16, succeeded-edge atomic completion lock):
  - `vizier-core/src/jobs/mod.rs` now executes `finalize_job_with_artifacts -> apply_workflow_routes -> scheduler_tick_locked` for `WorkflowNodeOutcome::Succeeded` inside one `SchedulerLock` critical section.
  - Success-route conversion is unchanged (`on.succeeded` remains context propagation; failed/blocked/cancelled routes remain retry-driven), and non-succeeded completion flow behavior is unchanged in this phase.
  - Succeeded completion now emits ordered debug traces for lock acquisition/finalization/route application/tick advancement/release to aid future race diagnosis.
  - Runtime coverage adds deterministic concurrent-tick pressure assertions plus a `worktree.prepare -> resolve_prompt -> invoke_agent` chain regression that checks non-null execution-context propagation across succeeded edges.
- Update (2026-02-15, hard-cut compatibility removal):
  - `vizier run <flow>` resolver is now canonical-only: explicit file/path, configured `[commands]` alias, or canonical selector (`template.name@vN`); implicit repo/global flow-name fallback discovery is removed.
  - Legacy selector/config bridges are hard-failed with migration guidance: dotted selectors (`template.name.vN`), `[workflow.templates]`, and legacy `[agents.<scope>]`.
  - Runtime/job bridges are hard-cut: `metadata.scope` and `workflow_capability_id` are no longer active metadata, runtime root resolution no longer falls back to `metadata.worktree_path`, and patch artifact handling is `command.patch`/`command_patch` only.
  - Snapshot path handling is canonical-only at `.vizier/narrative/snapshot.md`; legacy `.vizier/.snapshot` alias discovery is removed.
  - Docs and tests now target canonical-only behavior (`.vizier/workflows/*.toml`, selector `@vN`, explicit migration failures for legacy inputs).
- Update (2026-02-15, global default workflows):
  - `vizier run` flow resolution now includes implicit global alias lookup after repo-local sources, using `[workflow.global_workflows]` (`enabled` default true, optional `dir` override) and repo-bounded path safety that only allows out-of-repo file selectors under the resolved global workflow directory.
  - Repo fallback discovery now treats `.vizier/workflows/<flow>.{toml,json}` as canonical while retaining legacy `.vizier/workflow/*` compatibility.
  - Install surfaces now seed stage templates from `.vizier/workflows/{draft,approve,merge}.toml` into `WORKFLOWSDIR` (default `<base_config_dir>/vizier/workflows`), preserve pre-existing user templates, and keep uninstall parity via manifest tracking of installed/unchanged files.
  - Coverage added for config parsing defaults/overrides, resolver precedence and path safety, and install/uninstall manifest behavior for preserved templates.
- Update (2026-02-15, templates gate retry stabilization):
  - Repaired the gate-breaking regression in repo-local stage artifacts: `.vizier/config.toml` now restores `[commands].draft|approve|merge`, and `.vizier/workflow/{draft,approve,merge}.toml` is back on canonical `template.stage.*@v2` `cap.env.*`/`cap.agent.invoke`/`control.*` labels.
  - Draft-stage cleanup now uses an explicit `after` dependency from `stage_commit` (instead of a non-stop-gate `on.succeeded` route) to satisfy the `git.stage_commit` capability contract.
  - Merge-stage CI/CD gating now sources script config from the `cicd` gate definition only and removes the self-loop failed route, so gate failures settle as `failed` and operator recovery flows through `vizier jobs retry`.
  - Merge-stage runtime shape currently uses integrate/conflict/cicd gate node terminal statuses without an explicit template sink node.
  - Validation signal: previously failing run-stage tests (`stage_aliases`, `stage_jobs_control_paths`, `merge_stage_conflict_gate`) now pass, and `./cicd.sh` is green.
- Update (2026-02-15, templates worktree drift repair):
  - Repaired repo-local drift where `.vizier/config.toml` had dropped `[commands].draft|approve|merge` and stage aliases could fall back to unresolved legacy selectors.
  - Restored `.vizier/workflow/{draft,approve,merge}.toml` from legacy `template.stage.*@v1` + `vizier.*` labels to canonical `template.stage.*@v2` DAGs (`cap.env.*`, `cap.agent.invoke`, `control.*`) while keeping stage smoke node IDs (`persist_plan`, `stage_commit`, `stop_gate`, `merge_integrate`, `merge_gate_cicd`, `merge_conflict_resolution`).
  - Added explicit `slug` args on merge-stage nodes so conflict sentinels stay keyed to `.vizier/tmp/merge-conflicts/<slug>.json`.
  - Validation signal: targeted run-stage failures reproduced from gate output now pass, and `./cicd.sh` is green again.
- Update (2026-02-15, primitive stage-template cutover):
  - Repo-local stage templates `.vizier/workflow/{draft,approve,merge}.toml` now ship as canonical primitive DAGs (`template.stage.*@v2`) using only `cap.env.*`, `cap.agent.invoke`, and `control.*` identities.
  - Stage orchestration aliases are now explicit in repo config (`[commands].draft|approve|merge = "file:.vizier/workflow/<stage>.toml"`), with composed `develop` left as an optional higher-level flow.
  - Docs now describe stage execution as `vizier run` + `vizier jobs` only (`docs/user/workflows/alias-run-flow.md`, `docs/user/workflows/stage-execution.md`, `docs/dev/scheduler-dag.md`, `docs/dev/vizier-material-model.md`, `docs/user/config-reference.md`, `example-config.toml`).
  - Integration coverage in `tests/src/run.rs` now asserts stage alias smoke runs, approve stop-condition retry-loop attempts, stage job control paths (`approve/cancel/tail/attach/retry`), and merge conflict-gate sentinel behavior.
  - Validation gates were re-run and are green (`cargo check --all --all-targets`, `cargo test --all --all-targets`, `./cicd.sh`).
- Update (2026-02-15, execution-root propagation):
  - `vizier-core/src/jobs/mod.rs` now carries additive workflow metadata `execution_root` and resolves runtime roots by precedence (`execution_root` -> legacy `worktree_path` -> repo root) with repo-boundary canonicalization checks.
  - Runtime route handling now keeps `on.succeeded` topology unchanged (`after:success` bridge) while using explicit route metadata to propagate execution context edge-locally to downstream queued nodes; non-success retry routes now inject propagated context before scheduler requeue.
  - `worktree.prepare` now records execution-root context, successful `worktree.cleanup` resets execution root to `.` and clears worktree ownership metadata, and retry rewind mirrors that reset/preserve split for done/skipped vs degraded cleanup.
  - Jobs observability now exposes `execution_root` via `vizier jobs show` fields/json, and runtime/integration coverage now asserts propagation idempotence, running-target no-mutation guards, precedence/safety failures, and run-time successor propagation.
- Update (2026-02-15, `--set` Phase 1 expansion surface):
  - `vizier-cli/src/workflow_templates.rs` now expands `--set` queue-time across Phase 1 fields instead of args-only: artifact payload strings (`needs`/`produces`), lock keys, custom precondition args, gate script/custom fields, gate bool fields (`approval.required`, `cicd.auto_resolve`), retry mode/budget, and artifact-contract IDs/versions.
  - Queue-time coercion now validates expanded typed fields with field-path errors (bool tokens, retry budget `u32`, retry mode enum parse) before enqueue, preserving all-or-nothing manifest/job materialization.
  - Coverage expanded in `vizier-cli` unit tests and `tests/src/run.rs` integration tests for non-args expansion success and unresolved non-args no-partial-enqueue failure.
  - Phase 2 topology/identity interpolation (`after`/`on`, template `id/version`, imports, links) remains intentionally deferred pending a determinism decision.
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
