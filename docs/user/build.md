# `vizier build`

`vizier build` has two modes:

1. Create a build session from a TOML/JSON build file.
2. Execute a succeeded build session by queueing scheduler jobs from the compiled build-execute workflow template DAG (built-in default is `materialize -> approve -> review -> merge`, depending on pipeline).

For the canonical artifact/state contract behind these flows (build manifests, execution state, job/sentinel/session relationships, durability classes), see `docs/dev/vizier-material-model.md`.

## Queue file-backed intents with `vizier patch`

```bash
vizier patch <file...> [--pipeline approve|approve-review|approve-review-merge] [--target BRANCH] [--resume] [--yes] [--after JOB_ID ...] [--follow]
```

`vizier patch` is a low-ceremony wrapper over `vizier build` + `vizier build execute` for operators who already have one or more spec/intent files.

Behavior:
- Enqueues a root scheduler job (`command_alias=patch`, shown in the `Scope` field) immediately; use `--follow` to stream its logs.
- Validates every file inside that root job before queueing any build-execute phase jobs.
- Enforces in-repo paths, readable UTF-8 files, and non-empty content.
- Deduplicates repeated files by default while preserving first-seen order.
- Builds a deterministic patch session id from ordered files + effective pipeline/target and writes an internal build file under `.vizier/tmp/patches/`.
- Uses strict linear ordering (`step N` depends on `step N-1`) so execution follows CLI order exactly.
- Defaults to `approve-review-merge` when `--pipeline` is omitted (explicit `--pipeline` still wins).
- Reuses build pipelines (`approve`, `approve-review`, `approve-review-merge`) and supports `--resume` to reuse queued/running/succeeded phase jobs from the same patch session.
- Applies `--after <job-id>` to the first queued patch root job.
- Interactive runs prompt unless `--yes` is set; non-interactive runs require `--yes`.

Patch observability:
- Root jobs show `patch` in `vizier jobs show` (from `command_alias`, with legacy `scope` compatibility fallback).
- Root jobs also carry workflow-template compile metadata (`Workflow template`, `Workflow node`, `Workflow policy snapshot`) sourced from `[commands].patch` (legacy fallback: `[workflow.templates].patch`).
- `--follow` output includes a preflight block (`Patch session`, ordered file queue, pipeline/target, execution manifest path).
- Queued phase jobs carry `patch_file`, `patch_index`, and `patch_total` metadata in job records and `vizier jobs show`.

## Create a build session

```bash
vizier build --file path/to/build.toml [--name session-name]
```

- `--file` is required in create mode.
- `--name` is optional. When provided, it becomes the build id directly.
  - Uses the same name rules as draft plan names (letters/numbers/dashes, no `/`, no leading `.`).
  - Fails if `build/<name>` already exists or `.vizier/implementation-plans/builds/<name>/` already exists.
- Without `--name`, Vizier keeps auto-id allocation.

Create mode writes artifacts to:

```text
.vizier/implementation-plans/builds/<build_id>/
```

and commits them to `build/<build_id>` (unless `--no-commit` is active).

### Build file schema

Top-level `steps` array where each entry is either:
- A single intent object (`{ text = "..." }` or `{ file = "..." }`), or
- A parallel group (`[{...}, {...}]`).

Unknown keys are rejected.

### Intent fields

Each intent supports `text` or `file` (exactly one), plus optional build policy overrides:

- `profile`: build profile name from `[build.profiles.<name>]`.
- `pipeline`: `approve | approve-review | approve-review-merge`.
- `merge_target`: `primary | build | <branch-name>`.
- `review_mode`: `apply_fixes | review_only | review_file`.
- `skip_checks`: boolean (`review --skip-checks`).
- `keep_branch`: boolean (`merge --keep-branch`).
- `after_steps`: list of step keys (for example `["02a", "02b"]`) that must complete before this step can start.

### Example (TOML)

```toml
steps = [
  { text = "Build API foundations" },
  [
    { file = "intents/backend.md" },
    { file = "intents/frontend.md" },
  ],
  { text = "Finalize release readiness" },
]
```

## Execute a build session

```bash
vizier build execute <build-id> [--pipeline approve|approve-review|approve-review-merge] [--resume] --yes [--follow]
```

Defaults:
- Pipeline defaults to `[build].default_pipeline` (built-in default: `approve`) when `--pipeline` is omitted.
- Non-interactive runs require `--yes`.
- Interactive runs prompt unless `--yes` is set.

Execution mode:
- Loads `build/<build-id>` manifest.
- Requires manifest `status = succeeded`.
- Materializes deterministic per-step `draft/<slug>` branches and canonical plan docs.
- Compiles each run against the configured workflow template mapping (`[commands].build_execute`, legacy fallback `[workflow.templates].build_execute`) and queues every compiled node in topological `after` order.
- Prevalidates every compiled node capability contract before queueing any step jobs (loop wiring/cardinality + schedulable arg-shape checks), so invalid templates fail fast without partial graph creation.
- All queued build-execute nodes run through hidden `__workflow-node` jobs. Canonical capabilities (`cap.build.materialize_step`, `cap.plan.apply_once`, `cap.review.critique_or_fix`, `cap.git.integrate_plan_branch`) execute via node-runtime built-in handlers; generic custom capability nodes execute through generic node runtime.
- Persists execution state at:

```text
.vizier/implementation-plans/builds/<build_id>/execution.<resume-key>.json
```

`resume-key = default` uses `execution.json`.

### Build policy config (`[build]`)

Use config defaults to avoid repeating orchestration flags per run:

```toml
[build]
default_pipeline = "approve"
default_merge_target = "primary"
stage_barrier = "strict"
failure_mode = "block_downstream"
default_review_mode = "apply_fixes"
default_skip_checks = false
default_keep_draft_branch = false
default_profile = "integration"

[build.profiles.integration]
pipeline = "approve-review-merge"
merge_target = "build"
review_mode = "review_only"
skip_checks = true
keep_branch = true
```

Key behavior:
- `default_pipeline`: default phase path for steps.
- `default_merge_target`: default merge routing (`primary`, `build/<id>`, or explicit branch).
- `stage_barrier`: `strict` (implicit prior-stage dependencies) or `explicit` (dependencies only from `after_steps`).
- `failure_mode`: `block_downstream` or `continue_independent` (recorded in policy/output for audit and resume drift checks).
- `default_review_mode`, `default_skip_checks`, `default_keep_draft_branch`: defaults for review/merge flags.
- `default_profile`: profile applied when the step omits `profile`.
- `[build.profiles.<name>]`: reusable policy bundles; missing fields inherit from `[build]`.

### Policy precedence

Effective per-step policy resolves in this order:

1. CLI `--pipeline` (global override for this execute run).
2. Step inline overrides in the build file.
3. Step `profile` values from `[build.profiles.<name>]`.
4. `[build]` defaults.
5. Built-in defaults.

### Pipeline phases

- Built-in `template.build_execute@v1`:
  - `approve`: `materialize -> approve`
  - `approve-review`: `materialize -> approve -> review`
  - `approve-review-merge`: `materialize -> approve -> review -> merge`
- Custom `build_execute` templates may define arbitrary node IDs/kinds/edges; execution follows template `after` edges, not hard-coded phase IDs. Semantic dispatch is capability-first, so label aliases continue to work when they resolve to the same capability.

### Stage dependencies

- `stage_barrier = strict`: steps in stage `N` wait on terminal completion of all steps in stages `< N`.
- `stage_barrier = explicit`: stage barriers are removed; only `after_steps` dependencies apply.
- `after_steps` is always additive and supports cross-stage/cross-parallel dependencies by step key.

### Resume semantics

`--resume` reloads `execution.<resume-key>.json` and:
- Reuses existing queued/running/succeeded phase jobs.
- Re-enqueues missing or non-reusable failed/cancelled/blocked jobs.
- Uses template resume policy (`policy.resume`):
  - `reuse_mode = strict`: fail on any execution-policy drift.
  - `reuse_mode = compatible`: allow policy-only drift while still rejecting node/edge/artifact drift.
- Drift diagnostics are categorized as:
  - `node mismatch` for template/pipeline/node-set changes,
  - `edge mismatch` for dependency-graph drift,
  - `policy mismatch` for stage/failure/review/target behavior drift,
  - `artifact mismatch` for artifact-contract drift.

Running without `--resume` after execution state exists fails with guidance.

## Output

`vizier build execute` prints:
- Outcome (`Build execution queued` or `Build execution resumed`)
- Build id
- Pipeline override (if any)
- Stage barrier
- Failure mode
- Resume key
- Reuse mode
- Workflow template
- Policy snapshot hash
- Execution manifest path
- Optional failed step
- Step table with `Step`, `Slug`, `Branch`, `Pipeline`, `Target`, `Review mode`, `Deps`, `Jobs`, and `Status`

## Internal materialization job

`build execute` queues an internal scheduler entrypoint:

```bash
vizier build __materialize <build-id> --step <step-key> --slug <slug> --branch <draft-branch> --target <base-branch>
```

This command is hidden from normal operator usage.

Custom/shell/gate nodes (and non-primary wrapper-template nodes) use a hidden scheduler entrypoint:

```bash
vizier __workflow-node --scope <scope> --node <node-id> --slug <slug> --branch <draft-branch> --target <target-branch> --node-json '<serialized-node>'
```

`vizier build __template-node ...` remains as a build-specific compatibility shim and forwards into the same generic workflow-node runtime.

## Failure behavior

Preflight fails before enqueue when:
- Build id/branch is missing.
- Manifest is invalid or not `succeeded`.
- Referenced step plan artifacts are missing.
- Step policy is invalid (enum values, incompatible review/merge options for a chosen pipeline, missing profile).
- `after_steps` contains unknown keys or dependency cycles.
- Slug/branch allocation fails.

Runtime failures follow normal scheduler behavior:
- Failed phase jobs mark downstream dependencies as blocked.
- Execution state and job records preserve IDs/status for resume and auditing.
