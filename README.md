# Vizier

Vizier is a repository maintenance CLI for agent-driven workflows.

It compiles workflow templates into scheduler jobs, runs them in managed worktrees, and exposes job controls (`tail`, `attach`, `approve`, `retry`, `cancel`) through a single CLI.

Experimental project: expect interface changes.

## Current CLI Surface

Active top-level commands:

- `vizier help`
- `vizier init`
- `vizier list`
- `vizier jobs`
- `vizier run`
- `vizier completions`
- `vizier release`

`vizier cd` and `vizier clean` are still parsed but intentionally return deprecation errors.

Legacy wrapper commands like `vizier save`, `vizier draft`, `vizier approve`, `vizier review`, and `vizier merge` are removed.
Stage orchestration now runs through `vizier run <flow>`.

## Quick Start

Prerequisites:

- Rust toolchain (`cargo`)
- `git`
- `jq` (used by bundled progress filter scripts)
- An installed agent CLI (default selector is `codex`)

Install from this clone:

```bash
PREFIX="$HOME/.local" ./install.sh
```

Initialize a repository:

```bash
vizier init
# or CI-safe validation:
vizier init --check
```

Recommended alias map in `.vizier/config.toml`:

```toml
[commands]
draft = "file:.vizier/workflows/draft.hcl"
approve = "file:.vizier/workflows/approve.hcl"
merge = "file:.vizier/workflows/merge.hcl"
develop = "file:.vizier/develop.hcl"
```

Validate stage templates without enqueueing jobs:

```bash
vizier run draft --file specs/DEFAULT.md --name my-change --check
vizier run approve --set slug=my-change --check
vizier run merge --set slug=my-change --check
```

Run and follow a stage:

```bash
vizier run draft --file specs/DEFAULT.md --name my-change --follow
```

Inspect and control jobs:

```bash
vizier jobs list
vizier jobs show <job-id>
vizier jobs tail <job-id> --follow
vizier jobs approve <job-id>
vizier jobs retry <job-id>
vizier jobs cancel <job-id>
```

## Workflow Model

`vizier run <flow>` resolves workflows in this order:

1. Explicit file source (`file:<path>` or direct `.hcl`/`.toml`/`.json` path)
2. `[commands]` alias mapping from config
3. Canonical selector (`template.name@vN`)

Useful run controls:

- `--check` for validate-only preflight (no manifests, no enqueue)
- `--after <job-id|run:<run-id>>` for external dependencies
- `--repeat <N>` for strict serial repeat runs
- `--follow` for terminal-state streaming and deterministic exit codes

## Configuration

See:

- `example-config.toml`
- `docs/user/config-reference.md`
- `docs/user/prompt-config-matrix.md`
- `docs/user/workflows/alias-run-flow.md`

## Man Pages

Installed references:

- `man vizier`
- `man vizier-jobs`
- `man 5 vizier-config`
- `man 7 vizier-workflow`
- `man 7 vizier-workflow-template`

## Development

Project validation gate:

```bash
./cicd.sh
```
