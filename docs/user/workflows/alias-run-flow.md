# Alias-Run Flow

This page covers plan-workflow entry points that orchestrate multiple stages: `vizier build`, `vizier patch`, and `vizier run <alias>`.

For command-by-command stage behavior, see `docs/user/workflows/stage-execution.md`.

## Build and patch pipelines

When you want to batch related plan drafts, start with:

```bash
vizier build --file examples/build/todo.toml --name todo-batch
```

Create mode writes a build session to `build/<id>` with plan artifacts under `.vizier/implementation-plans/builds/<id>/`.
To execute that session, run:

```bash
vizier build execute todo-batch --yes
```

Execution mode queues per-step scheduler jobs from the compiled build-execute template (built-in default materializes canonical `draft/<slug>` plan docs, then runs `approve`/`review`/`merge` according to resolved policy).

By default, pipeline/target/review behavior comes from `[build]` config and optional step-level overrides in the build file. Pass `--pipeline ...` only for one-off overrides.
Use `--resume` to continue from the template-selected execution lane (`execution.<resume-key>.json`), reusing non-terminal jobs and applying template `policy.resume.reuse_mode` drift rules.

For build schema, execute options, and artifact details, see:

- `docs/user/build.md`
- `examples/build/todo.toml`
- `examples/build/todo.json`

If your inputs are already discrete files and you want a thinner wrapper:

```bash
vizier patch BUG1.md BUG2.md --yes
```

`patch` queues a root scheduler job first, runs full-file preflight inside that job, then queues per-step phase jobs in exact CLI order.
It deduplicates repeated paths by default, defaults to the full `approve-review-merge` pipeline when `--pipeline` is omitted, and reuses build execution machinery under a deterministic patch session id.

## Compose a one-command plan flow with `vizier run`

If your repo defines a composed alias (for example `develop`) under `[commands]`, run the full draft/approve/review/merge chain as one queued DAG:

```toml
[commands]
develop = "file:.vizier/develop.toml"
```

```bash
vizier run develop --name release-cut "ship release cut"
```

`vizier run` resolves `[commands.<alias>]` first, then repo fallback files when the alias is unmapped:

- `.vizier/<alias>.toml`
- `.vizier/<alias>.json`
- `.vizier/workflow/<alias>.toml`
- `.vizier/workflow/<alias>.json`

The run keeps scheduler semantics (`--after`, locks, dependencies, retries, `--follow`) and emits standard job metadata per staged node.

## Agent configuration for plan commands and alias runs

Plan commands (`vizier draft`, `vizier approve`, `vizier review`, `vizier merge`) and alias runs (`vizier run <alias>`) resolve agent settings through command aliases plus template selectors.

Declare defaults under `[agents.default]`, then override per alias with `[agents.commands.<alias>]` (and optionally per template with `[agents.templates."<selector>"]`):

```toml
[agents.default]
agent = "codex"

[agents.commands.approve]
agent = "codex"

[agents.commands.review]
agent = "codex"

[agents.commands.merge]
agent = "gemini"
```

For the full catalog of knobs and defaults (aliases, template selectors, precedence, CLI overrides), see `docs/user/config-reference.md`.
Use `vizier plan --json` to inspect resolved per-command alias settings before running workflow commands.

Config precedence: when `--config-file` is omitted, Vizier loads user/global config as a base and overlays `.vizier/config.toml` so repo settings override while missing keys inherit your defaults.
`VIZIER_CONFIG_FILE` is only used when neither config file exists.

If the selected agent fails, the command fails immediately. Vizier does not auto-fallback to another selector.

CLI selector overrides (`--agent`) apply only to the current command invocation and sit above alias/template config tables.

Want to exercise an alias-resolved agent without touching `.vizier` or Git?
Use `vizier test-display [--command save|draft|approve|review|merge|patch|build_execute]`.
It streams progress through the normal display stack using a harmless prompt and reports exit code/duration/stdout/stderr.
