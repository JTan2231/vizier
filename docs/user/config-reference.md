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

## Operational Commands

Current user-facing commands are:

- `vizier init`
- `vizier list`
- `vizier cd`
- `vizier clean`
- `vizier jobs`
- `vizier completions`
- `vizier release`

## Canonical Companion Docs

- `docs/user/prompt-config-matrix.md`
- `docs/user/workflows/draft-approve-merge.md`
