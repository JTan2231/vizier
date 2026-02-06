# `vizier build`

`vizier build` has two modes:

1. Create a build session from a TOML/JSON build file.
2. Execute a succeeded build session by queueing scheduler jobs (`materialize -> approve -> review -> merge`, depending on pipeline).

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
- Queues phase jobs through scheduler using existing `approve`/`review`/`merge` commands.
- Persists execution state at:

```text
.vizier/implementation-plans/builds/<build_id>/execution.json
```

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

- `approve`: `materialize -> approve`
- `approve-review`: `materialize -> approve -> review`
- `approve-review-merge`: `materialize -> approve -> review -> merge`

### Stage dependencies

- `stage_barrier = strict`: steps in stage `N` wait on terminal completion of all steps in stages `< N`.
- `stage_barrier = explicit`: stage barriers are removed; only `after_steps` dependencies apply.
- `after_steps` is always additive and supports cross-stage/cross-parallel dependencies by step key.

### Resume semantics

`--resume` reloads `execution.json` and:
- Reuses existing queued/running/succeeded phase jobs.
- Re-enqueues missing or non-reusable failed/cancelled/blocked jobs.
- Fails if resolved policy drifts from existing execution state (pipeline, target routing, review mode, checks/branch retention, dependencies, or barrier/failure settings).

Running without `--resume` after execution state exists fails with guidance.

## Output

`vizier build execute` prints:
- Outcome (`Build execution queued` or `Build execution resumed`)
- Build id
- Pipeline override (if any)
- Stage barrier
- Failure mode
- Execution manifest path
- Optional failed step
- Step table with `Step`, `Slug`, `Branch`, `Pipeline`, `Target`, `Review mode`, `Deps`, `Jobs`, and `Status`

## Internal materialization job

`build execute` queues an internal scheduler entrypoint:

```bash
vizier build __materialize <build-id> --step <step-key> --slug <slug> --branch <draft-branch> --target <base-branch>
```

This command is hidden from normal operator usage.

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
