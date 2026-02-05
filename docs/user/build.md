# `vizier build`

`vizier build` reads a JSON or TOML build file and schedules one or more `vizier draft` runs in the order described. It is intended for batching plan drafts while keeping the intent docs as plain prose.

## Command

```bash
vizier build --file path/to/build.toml
```

The file extension selects the parser:
- `.toml` → TOML
- `.json` → JSON

## Build File Schema

The build file must contain a top-level `steps` array. Each entry is either:
- An intent doc object with **exactly one** of `text` or `file`, or
- An array of intent doc objects to run in parallel.

Unknown keys are errors.

### Intent doc fields
- `text` (string): inline intent content.
- `file` (string): path to a file containing the intent content.

### Examples

TOML:

```toml
steps = [
  { text = "Implement X with Y constraints and tests." },
  [
    { file = "intents/feature-a.md" },
    { file = "intents/feature-b.md" },
  ],
  { text = """Finalize integration plan and test strategy.""" },
]
```

JSON:

```json
{
  "steps": [
    { "text": "Implement X with Y constraints and tests." },
    [
      { "file": "intents/feature-a.md" },
      { "file": "intents/feature-b.md" }
    ],
    { "text": "Finalize integration plan and test strategy." }
  ]
}
```

### Path Resolution

`file` paths are resolved relative to the directory containing the build file. Resolved paths must remain inside the repository root.

## Execution Semantics

- `steps` execute in order.
- A single intent doc runs serially.
- A parallel group runs concurrently where possible (subject to scheduler locks/pinned-head limits).
- Each intent doc is executed by `vizier draft` using the resolved text.
- `build` does **not** auto-run `approve`, `review`, or `merge`.

## Errors

`vizier build` fails fast on:
- Missing or empty `steps`.
- Empty intent content (after trimming).
- Invalid schema or unknown keys.
- Intent files that resolve outside the repository.

## Output

The command prints a short summary with the step index, plan slug, draft branch, and job id, then points to `vizier jobs list` and `vizier jobs schedule` for inspection.
