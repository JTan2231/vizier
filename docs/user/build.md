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

- `text`: inline intent text.
- `file`: path to intent text file, resolved relative to the build file directory and constrained to repo root.

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
- Pipeline defaults to `approve`.
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

### Pipeline phases

- `approve`: `materialize -> approve`
- `approve-review`: `materialize -> approve -> review`
- `approve-review-merge`: `materialize -> approve -> review -> merge`

### Stage dependencies

Build stage ordering is preserved:
- Steps in stage `N` wait on terminal phase completion of all steps in stages `< N`.
- Parallel siblings in the same stage run independently.

### Resume semantics

`--resume` reloads `execution.json` and:
- Reuses existing queued/running/succeeded phase jobs.
- Re-enqueues missing or non-reusable failed/cancelled/blocked jobs.
- Fails if pipeline does not match existing execution state.

Running without `--resume` after execution state exists fails with guidance.

## Output

`vizier build execute` prints:
- Outcome (`Build execution queued` or `Build execution resumed`)
- Build id
- Pipeline
- Execution manifest path
- Optional failed step
- Step table with `Step`, `Slug`, `Branch`, `Jobs`, and `Status`

## Internal materialization job

`build execute` queues an internal scheduler entrypoint:

```bash
vizier build __materialize <build-id> --step <step-key> --slug <slug> --branch <draft-branch>
```

This command is hidden from normal operator usage.

## Failure behavior

Preflight fails before enqueue when:
- Build id/branch is missing.
- Manifest is invalid or not `succeeded`.
- Referenced step plan artifacts are missing.
- Slug/branch allocation fails.

Runtime failures follow normal scheduler behavior:
- Failed phase jobs mark downstream dependencies as blocked.
- Execution state and job records preserve IDs/status for resume and auditing.
