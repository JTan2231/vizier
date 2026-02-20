# Command Entry Points

This page summarizes command entry points, including the enqueue front-door `vizier run` and the read-only inspection command `vizier audit`.

## Repository Setup

Run `vizier init` once per repository (or `vizier init --check` in CI) to ensure required scaffold files and `.gitignore` coverage exist. The scaffold includes `.vizier/config.toml`, `.vizier/workflows/{draft,approve,merge,commit}.hcl`, `.vizier/prompts/{DRAFT,APPROVE,MERGE,COMMIT}_PROMPTS.md`, and a root `ci.sh` stub used by the default merge gate config.

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
- `vizier run draft --spec-file specs/DEFAULT.md --slug my-change --follow`
- `vizier run draft --file specs/DEFAULT.md --name my-change --follow`
- `vizier run draft specs/DEFAULT.md my-change draft/my-change --follow`
- `vizier run approve --set slug=my-change --set branch=draft/my-change --follow`
- `vizier run merge --set slug=my-change --set branch=draft/my-change --set target_branch=master --follow`
- `vizier run develop`
- `vizier run file:.vizier/workflows/custom.hcl --set key=value`
- `vizier run develop --after <job-id> --require-approval`
- `vizier run develop --after run:<run-id>`
- `vizier run develop --repeat 3`
- `vizier run develop --repeat 2 --follow --format json`
- `vizier run develop --follow --format json`
- `vizier run develop --check`
- `vizier run file:.vizier/workflows/custom.hcl --check --set key=value --format json`

Recommended repo alias map:

```toml
[commands]
draft = "file:.vizier/workflows/draft.hcl"
approve = "file:.vizier/workflows/approve.hcl"
merge = "file:.vizier/workflows/merge.hcl"
develop = "file:.vizier/develop.hcl"
```

Resolution order for `vizier run <flow>` is: explicit file source, configured `[commands]` alias, then selector identity lookup (`template.name@vN`). There is no implicit repo/global `<flow>` fallback discovery.

`[workflow.global_workflows]` only controls whether explicit file selectors are allowed to resolve outside the repo root under the configured global workflows directory.

HCL templates should author queue-time placeholders as `$${key}` (escaped for HCL). Vizier receives `${key}` after HCL decoding and applies normal queue-time expansion.

Workflow parameter input styles:

- Named flags: unknown `--long-flag` inputs on `vizier run` are treated as template params (`--spec-file` maps to `spec_file`).
- Template aliases: `[cli].named` can map friendly labels to canonical params (`--name` -> `slug`, `--file` -> `spec_file` in stage draft).
- Ordered inputs: extra positional values after `<flow>` map using template `[cli].positional` order.
- Explicit `--set key=value` remains supported and keeps last-write-wins behavior.
- `--repeat <N>` is run-local (`N >= 1`, default `1`): Vizier enqueues `N` runs with unique run IDs and chains repeat iteration `i>1` on iteration `i-1` success sinks (`run:<prev_run_id>` expansion) for deterministic serial execution.
- Stage draft now snapshots `spec_file` contents into `persist_plan.args.spec_text` at enqueue time when `spec_source=inline` and `spec_text` is empty, so the spec file does not need to be committed into the stage worktree.
- For stage templates, `worktree_prepare` derives `branch=draft/<slug>` when `branch` is omitted.
- Stage templates use repo-local prompt files under `.vizier/prompts/` (`DRAFT_PROMPTS.md`, `APPROVE_PROMPTS.md`, `MERGE_PROMPTS.md`) so draft/approve runs do not require `prompt_text` overrides.
- Prompt files can declare runtime placeholders as `{{...}}`. `prompt.resolve` requires all placeholders to resolve from node args (`{{key}}`), run-manifest node args (`{{node_id.arg_key}}`), or file includes (`{{file:path}}`).
- Executor arg contracts are validated before enqueue, and root-node preflight now prints entry-input guidance when required args are missing; current required-input checks include `worktree.prepare` (`branch|slug|plan`), `git.integrate_plan_branch` (`branch|source_branch|plan_branch|slug|plan`), `cicd.run` (`command/script` or a non-empty cicd gate script), and `patch.pipeline_prepare`/`patch.execute_pipeline` (`files_json`).

Queue-time `--set` expansion now applies beyond `nodes.args` to artifact payloads, lock keys, custom precondition args, gate fields, retry policy, and artifact-contract IDs/versions. Unresolved placeholders and invalid coercions fail before enqueue (no partial manifests/jobs). Topology/identity expansion (`after`, `on`, template/import/link identity) remains deferred.

Runtime nodes now follow one I/O contract behind `vizier __workflow-node`: lifecycle/progress diagnostics on `stderr`, operational output on `stdout`, and a persisted `vizier.operation_output.v1` payload under `.vizier/jobs/artifacts/data/...`. Each node implicitly publishes `custom:operation_output:<node_id>` for downstream `needs` + `read_payload(...)` consumption patterns.

`--after` accepts either direct job ids or grouped run references (`run:<run_id>`). Run references expand to the previous run's success-terminal sink job ids before normal scheduler dependency validation.

Use `vizier run --check` for validate-only preflight (queue-time checks only): flow resolution, template load/composition, parameter expansion/coercion, entry input checks, capability validation, and per-node compile checks all run, but Vizier does not create run manifests, enqueue jobs, or tick the scheduler. `--check` conflicts with enqueue/runtime flags: `--follow`, `--after`, `--require-approval`, `--no-require-approval`, and `--repeat`.

## Workflow Audit

Use `vizier audit <flow>` for queue-time artifact wiring inspection without enqueue/runtime side effects:

- `vizier audit develop`
- `vizier audit file:.vizier/workflows/custom.hcl --set key=value --format json`
- `vizier audit develop --strict`

`vizier audit` runs the same queue-time preprocessing path as `vizier run --check` (flow resolution, input mapping, `--set` expansion/coercion, plan `spec_file` inlining, capability validation), then reports:

- `output_artifacts` (stable, deduped union of produced artifacts, including implicit `custom:operation_output:<node_id>`)
- `output_artifacts_by_outcome` (`succeeded|failed|blocked|cancelled`)
- `untethered_inputs` (artifacts referenced in `needs` with no in-template producer, plus consumer node ids)

Exit behavior:

- `0`: audit succeeded
- `1`: resolution/parse/validation failure
- `10`: audit succeeded with untethered inputs and `--strict`

`vizier run` remains the only enqueue/execution front door.

## Release Flow

Use `vizier release --dry-run` to preview version/tag/notes and `vizier release --yes` to create artifacts.

## Shell Completions

Use `vizier completions <bash|zsh|fish|powershell|elvish>` to generate completion scripts.
