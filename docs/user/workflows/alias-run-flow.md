# Command Entry Points

This page summarizes command entry points, including the restored orchestration front-door `vizier run`.

## Repository Setup

Run `vizier init` once per repository (or `vizier init --check` in CI) to ensure required marker files and `.gitignore` coverage exist.

## Pending Plan Visibility

Use `vizier list` to inspect pending `draft/*` branches and associated summaries.

## Job Operations

Use `vizier jobs` for scheduler/job records:

- `vizier jobs list`
- `vizier jobs schedule [--watch]`
- `vizier jobs show <job>`
- `vizier jobs tail <job> [--follow]`
- `vizier jobs attach <job>`
- `vizier jobs approve|reject|retry|cancel|gc ...`

## Workflow Run Orchestrator

Use `vizier run <flow>` to compile and enqueue repo-local workflow templates through scheduler primitives:

- `vizier run draft --set slug=my-change --set spec_text="..." --follow`
- `vizier run approve --set slug=my-change --set branch=draft/my-change --follow`
- `vizier run merge --set slug=my-change --set branch=draft/my-change --set target_branch=master --follow`
- `vizier run develop`
- `vizier run file:.vizier/workflow/custom.toml --set key=value`
- `vizier run develop --after <job-id> --require-approval`
- `vizier run develop --follow --format json`

Recommended repo alias map:

```toml
[commands]
draft = "file:.vizier/workflow/draft.toml"
approve = "file:.vizier/workflow/approve.toml"
merge = "file:.vizier/workflow/merge.toml"
develop = "file:.vizier/develop.toml"
```

Queue-time `--set` expansion now applies beyond `nodes.args` to artifact payloads, lock keys, custom precondition args, gate fields, retry policy, and artifact-contract IDs/versions. Unresolved placeholders and invalid coercions fail before enqueue (no partial manifests/jobs). Topology/identity expansion (`after`, `on`, template/import/link identity) remains deferred.

## Release Flow

Use `vizier release --dry-run` to preview version/tag/notes and `vizier release --yes` to create artifacts.

## Shell Completions

Use `vizier completions <bash|zsh|fish|powershell|elvish>` to generate completion scripts.
