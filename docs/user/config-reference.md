# Config Reference

This file describes the current, supported Vizier configuration posture.

## Load Order

Effective settings are resolved in this order:

1. CLI flags for the current run.
2. Repo config (`.vizier/config.toml` or `.vizier/config.json`).
3. Global config (`$XDG_CONFIG_HOME/vizier/config.toml` or platform equivalent).
4. `VIZIER_CONFIG_FILE` fallback (used only when repo/global config files are absent).

## Active Global Flags

- `-v` / `-vv`
- `-q`
- `-d`
- `--no-ansi`
- `-l, --load-session <id>`
- `-n, --no-session`
- `-C, --config-file <path>`

Legacy workflow-global flags are no longer supported.

## Common Tables

- `[display]`: output formatting defaults for list/jobs views.
- `[jobs]`: cancellation and retention behavior for job operations.
- `[commits]`: release/commit metadata formatting controls.
- `[commands]`: alias-to-template mapping consumed by `vizier run <alias>`.
- `[workflow.global_workflows]`: allowlist for explicit workflow file selectors outside the repo root.
- `[agents.default]`, `[agents.commands.<alias>]`, `[agents.templates."<selector>"]`: agent/prompt/runtime overrides.

`vizier run <flow>` accepts only:
- explicit `file:<path>` or direct `.hcl` path inputs (legacy `.toml`/`.json` templates still load during migration),
- configured `[commands]` aliases,
- canonical selectors (`template.name@vN`).

Legacy dotted selectors (`template.name.vN`), legacy `[workflow.templates]`, and implicit repo/global `<flow>` fallback discovery are unsupported and fail with migration guidance.
For HCL-authored templates, escape literal queue-time placeholders as `$${key}`.

Recommended stage aliases:

```toml
[commands]
draft = "file:.vizier/workflows/draft.hcl"
approve = "file:.vizier/workflows/approve.hcl"
merge = "file:.vizier/workflows/merge.hcl"
develop = "file:.vizier/develop.hcl"
```

Global workflow defaults:

```toml
[workflow.global_workflows]
enabled = true
dir = ""  # optional override; empty = <base_config_dir>/vizier/workflows
```

`[workflow.global_workflows]` does not add implicit alias discovery; it only allows explicit file selectors that resolve under that directory.

`<base_config_dir>` resolution order: `VIZIER_CONFIG_DIR`, `XDG_CONFIG_HOME`, `APPDATA`, `HOME/.config`, `USERPROFILE/AppData/Roaming`.

## Operational Commands

Current user-facing commands are:

- `vizier init`
- `vizier list`
- `vizier cd`
- `vizier clean`
- `vizier jobs`
- `vizier run`
- `vizier audit`
- `vizier completions`
- `vizier release`

## `vizier run --set` Expansion Surface

`vizier run <flow> --set key=value` applies queue-time interpolation after template composition (`imports` + `links`) and after defaults from `[params]` are merged.

Run-local orchestration controls include:

- `--after <job_id|run:<run_id>>` (repeatable)
- `--require-approval` / `--no-require-approval`
- `--repeat <N>` (default `1`, valid values `>= 1`)
- `--follow`

- `vizier run <flow> --param value` is accepted for workflow params; kebab-case flag names are normalized to snake_case keys (`--spec-file` => `spec_file`).
- Templates may define `[cli].named` aliases so friendly entry flags map to canonical params (`--name` => `slug`, `--file` => `spec_file` for stage draft).
- `vizier run <flow> <value...>` is accepted when the template defines `[cli].positional = ["param_a", "param_b", ...]`.
- `--set` remains last-write-wins by key.
- Phase 1 interpolation coverage includes:
  - `nodes.args.*`
  - artifact payload strings in `nodes.needs` and `nodes.produces.*`
  - `nodes.locks[].key`
  - `nodes.preconditions` custom-arg string values
  - gate script/custom string values and gate bool fields (`approval.required`, `cicd.auto_resolve`)
  - `nodes.retry.mode` / `nodes.retry.budget`
  - `artifact_contracts[].id` / `artifact_contracts[].version`
- Typed coercion is strict at queue time:
  - bool accepts `true|false|1|0|yes|no|on|off` (case-insensitive)
  - retry budget accepts decimal `u32` only
  - retry mode accepts canonical enum values (for example `never`, `on_failure`, `until_gate`)
- Unresolved placeholders or invalid coercions fail before enqueue; no run manifest or node jobs are created.
- Phase 2 topology/identity expansion (`nodes.after`, `nodes.on.*`, template `id/version`, `imports`, `links`) is intentionally deferred.

`vizier run --repeat <N>` enqueues the same resolved flow `N` times in strict sequence. Iteration `i>1` depends on iteration `i-1` success sinks (equivalent to appending `--after run:<previous_run_id>` internally), so repeats do not run in parallel.

## `vizier audit` Read-only Analysis

`vizier audit <flow>` uses the same flow resolution and queue-time preprocessing path as `vizier run --check`:

- explicit file/path, configured `[commands]` alias, or canonical selector (`template.name@vN`)
- positional input mapping + `--set key=value` expansion/coercion
- stage `plan.persist` `spec_file` inlining and capability validation

Audit output reports produced artifacts (including implicit `custom:operation_output:<node_id>`) and untethered `needs` artifacts with consumer node IDs. It does not write run manifests, enqueue jobs, or tick the scheduler.

## Workflow Dependency Policy

Workflow templates can opt into optimistic dependency-derived scheduling for missing artifact producers:

```hcl
policy = {
  dependencies = {
    missing_producer = "wait" # default is "block"
  }
}
```

- `block`: missing artifact with no known producer transitions the job to `blocked_by_dependency`.
- `wait`: missing artifact with no known producer keeps the job in `waiting_on_deps` with `awaiting producer for <artifact>`.
- This policy affects artifact dependencies only (`nodes.needs` -> `schedule.dependencies`), not explicit `--after` dependencies.

## Canonical Companion Docs

- `docs/user/prompt-config-matrix.md`
- `docs/user/workflows/draft-approve-merge.md`
