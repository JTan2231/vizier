# `vizier build`

`vizier build` runs a **build session** from a JSON or TOML file. Instead of queueing standalone `vizier draft` jobs, it creates one dedicated `build/<id>` branch, drafts each step in order, and writes a self-contained session artifact set under `.vizier/implementation-plans/builds/<id>/`.

## Command

```bash
vizier build --file path/to/build.toml
```

The file extension selects the parser:
- `.toml` -> TOML
- `.json` -> JSON

## Build File Schema

The build file must contain a top-level `steps` array. Each entry is either:
- An intent object with **exactly one** of `text` or `file`, or
- An array of intent objects to run as a parallel stage.

Unknown keys are errors.

### Intent fields
- `text` (string): inline intent content.
- `file` (string): path to a file containing intent content.

### Example (TOML, inline intents)

```toml
steps = [
  { text = "Build a basic TODO API in Rust with create/list/complete/delete endpoints, an in-memory store, and integration tests." },
  [
    { text = "Build a simple TODO web UI with list/add/complete interactions and clear empty/loading/error states." },
    { text = "Add CLI smoke checks and local run docs for the TODO app so contributors can validate API + UI together." },
  ],
  { text = "Finalize TODO app integration details, polish docs, and capture release-readiness checks before merge." },
]
```

### Example (JSON, file-based intents)

```json
{
  "steps": [
    { "text": "Design ingestion and validation flow" },
    [
      { "file": "intents/api.md" },
      { "file": "intents/worker.md" }
    ],
    { "text": "Finalize rollout and testing plan" }
  ]
}
```

Repository copies of the inline-intent TODO example are available at `examples/build/todo.toml` and `examples/build/todo.json`.

## Path Resolution

`file` paths are resolved relative to the directory that contains the build file. Resolved paths must stay inside the repository root.

## Session Lifecycle

`vizier build`:
1. Detects the target branch (primary branch detection).
2. Creates a `build/<id>` branch from that target.
3. Creates a temporary worktree on the build branch.
4. Executes build-mode drafting per step.
5. Writes session artifacts and commits them to the build branch.
6. Cleans up the temp worktree on success.

On failure, the build branch and session artifacts are preserved for inspection.

## Artifact Layout

Each run writes:

```text
.vizier/implementation-plans/builds/<build_id>/
  input/<build-file>
  plans/<step_key>-<slug>.md
  manifest.json
  summary.md
```

`manifest.json` tracks step order, per-step output paths, prior-plan references (`reads`), and run status (`running`/`succeeded`/`failed`).

## Context Rules

- Steps can reference plans from prior completed stages.
- Parallel siblings in the same stage do not read each other.
- Build-mode prompts include a compact prior-plan reference index by default (not full prior-plan body inlining).

## Errors

`vizier build` fails fast on:
- Missing or empty `steps`.
- Empty intent content.
- Invalid schema or unknown keys.
- Intent files outside repo root.
- Agent backend failure while drafting a step.

When a step fails, the manifest is marked `failed`, completed artifacts are preserved, and the command exits non-zero.

## Output

The command prints:
- Session outcome (`ready`, `pending`, or `failed`).
- Build id, build branch, and manifest path.
- Per-step status table (`Step`, `Status`, `Plan`, `Reads`).
