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
- `[workflow.global_workflows]`: default-on implicit alias lookup for global workflow files.
- `[workflow.templates]`: compatibility fallback for legacy selector lookups.

Recommended stage aliases:

```toml
[commands]
draft = "file:.vizier/workflows/draft.toml"
approve = "file:.vizier/workflows/approve.toml"
merge = "file:.vizier/workflows/merge.toml"
develop = "file:.vizier/develop.toml"
```

Global workflow defaults:

```toml
[workflow.global_workflows]
enabled = true
dir = ""  # optional override; empty = <base_config_dir>/vizier/workflows
```

`<base_config_dir>` resolution order: `VIZIER_CONFIG_DIR`, `XDG_CONFIG_HOME`, `APPDATA`, `HOME/.config`, `USERPROFILE/AppData/Roaming`.

## Operational Commands

Current user-facing commands are:

- `vizier init`
- `vizier list`
- `vizier cd`
- `vizier clean`
- `vizier jobs`
- `vizier run`
- `vizier completions`
- `vizier release`

## `vizier run --set` Expansion Surface

`vizier run <flow> --set key=value` applies queue-time interpolation after template composition (`imports` + `links`) and after defaults from `[params]` are merged.

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

## Canonical Companion Docs

- `docs/user/prompt-config-matrix.md`
- `docs/user/workflows/draft-approve-merge.md`
