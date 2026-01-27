# Vizier configuration reference

This file is the authoritative catalogue of Vizier’s configuration levers, their defaults, and how CLI flags override them. Pair it with `vizier plan` (or `vizier plan --json`) to inspect the fully resolved configuration for your current repo + global settings + CLI overrides.

## How configuration is loaded
- CLI flags have the last word; use them for one-off overrides.
- Without `--config-file`, Vizier overlays `~/.config/vizier/config.toml` (or platform equivalent) with `.vizier/config.toml`/`.json` in the repo; missing keys inherit from the lower layer.
- `--config-file <path>` (or `--config-file=<path>`) replaces the search. If no config files are found, Vizier falls back to `$VIZIER_CONFIG_FILE` when it points at an existing file.
- `VIZIER_CONFIG_DIR`/`XDG_CONFIG_HOME`/`APPDATA`/`HOME`/`USERPROFILE` influence the global config location; `VIZIER_AGENT_SHIMS_DIR` can point at bundled agent shims when you relocate them.
- Use `vizier plan --json` to see the merged config (per-command agent selection, prompt profiles, gate settings) before running draft/refine/approve/review/merge.

## Quick-start scenarios
- Pin review to Gemini while leaving other scopes on Codex: set `[agents.review] agent = "gemini"` (optionally override `[agents.review.agent].command` if you moved the shim) and run `vizier plan --json` to confirm the resolved selector before `vizier review`. CLI override: `vizier review --agent gemini` for one-offs.
- Tighten merge CI/CD gates: set `[merge.cicd_gate] script = "./cicd.sh"`, `retries = 3`, and (optionally) `auto_resolve = true` so Vizier retries failures up to three times with agent remediation. Override per run with `--cicd-script`, `--cicd-retries`, and `--auto-cicd-fix/--no-auto-cicd-fix`.
- Disable auto-commit for inspection: set `[workflow] no_commit_default = true` to hold assistant edits dirty/staged across ask/save/draft/refine/approve/review. For a single run, pass `--no-commit`; re-run without it before merging so history is finalized.
- Background execution: `[workflow.background]` controls whether assistant-backed commands default to background jobs. Use `--no-background` to force foreground execution, `--follow` to attach to background logs, or `--background` to detach explicitly (requires non-interactive flags for prompts). Commands that read input from stdin (for example, `vizier ask` with piped input) run in the foreground; `--background`/`--follow` require MESSAGE/`--file` input instead.
- Swap prompt text for a single scope: add `[agents.merge.prompts.merge_conflict] path = ".vizier/MERGE_CONFLICT_PROMPT.md"` (or `text = """..."""`) to override just merge-conflict prompting without touching other commands.

## Override matrix (config vs CLI)
- Agent selector/runtime: `[agents.<scope>] agent` picks the shim (`codex`, `gemini`, or a custom name) and `[agents.<scope>.agent]` customizes the runtime command/filter/output; CLI `--agent`, `--agent-command`, `--agent-label` override all scopes for the current run.
- Prompt selection: `[agents.<scope>.prompts.<kind>]` (`text`/`path` + nested `[agent]` overrides) → legacy `[prompts.*]` → `.vizier/*PROMPT*.md` → baked defaults; no CLI flag exists. Inspect with `vizier plan --json`.
- Workflow hold: `[workflow].no_commit_default` (default false) ↔ CLI `--no-commit` flag.
- Background posture: `[workflow.background].{enabled,quiet}` (default true/true) controls background-by-default execution for assistant commands; use `--no-background` for foreground, `--follow` to attach, or `--background` to detach explicitly (requires non-interactive flags).
- Merge gates: `[merge.cicd_gate].{script,retries,auto_resolve}` ↔ CLI `--cicd-script`, `--cicd-retries`, `--auto-cicd-fix/--no-auto-cicd-fix`.
- Merge history: `[merge].squash` / `[merge].squash_mainline` ↔ CLI `--squash`/`--no-squash`, `--squash-mainline`.
- Display/help: pager defaults are TTY-only; use `--pager`/`--no-pager` or `$VIZIER_PAGER` to force/disable. `--no-ansi` strips color; `-q/-v/-vv` control verbosity.
- Checks/review: `[review.checks].commands` ↔ CLI `vizier review --skip-checks` (to skip) or config to set the commands; merge CI/CD gate is reused during review with auto-fix disabled.

## Agents, prompts, and documentation toggles
- `agent` (root or `[agents.default]`): selector for the bundled shim (`codex` by default, `gemini` as the alternate) or any custom shim name you’ve installed. Unsupported `agent = "wire"` entries and `fallback_backend` keys are rejected.
- Agent runtime overrides: `[agents.<scope>.agent]` provide `command` (custom script), optional `progress_filter`, `output` (`auto`/wrapped JSON), and `enable_script_wrapper` (wraps non-shim scripts). Defaults are inferred from the selector; drop down to this table only when you need to point at a non-bundled script.
- Per-scope selector overrides: `[agents.ask|save|draft|refine|approve|review|merge] agent="<selector>"`; CLI `--agent/--agent-label/--agent-command` override all scopes for the current run.
- Documentation prompt toggles: `[agents.<scope>.documentation]` with `enabled` (default true), `include_snapshot` (default true), and `include_narrative_docs` (default true). Disable or trim context for conflict auto-resolve or other low-context flows. Narrative context now comes exclusively from `.vizier/narrative/` (including `.vizier/narrative/glossary.md` and thread docs); legacy `.vizier/todo_*.md` files are no longer read.
- Prompt text and per-prompt agent overrides: `[agents.<scope>.prompts.<kind>]` sets `text`/`path` plus nested `[agent]` runtime overrides for that scope+kind; fall back to `[prompts.<scope>]`, `.vizier/*.md` prompt files, `[prompts]`, then baked-in defaults. Prompt kinds: `documentation`, `commit`, `implementation_plan`, `plan_refine`, `review`, `merge_conflict` (see `docs/prompt-config-matrix.md` for the scope×kind map).
- Progress filters attach by selector: when `progress_filter` is unset, Vizier looks for a bundled `filter.sh` under the configured selector (Codex, Gemini, or any custom shim with a sibling filter) and wires it automatically.

## Workflow and gate settings
- Workflow defaults: `[workflow].no_commit_default` (default false) pairs with `--no-commit` to hold assistant edits for manual review across ask/save/draft/refine/approve/review.
- Background jobs: `[workflow.background].enabled` defaults assistant-backed commands to background runs; `--background` detaches explicitly, `--follow` tails logs, and `--no-background` restores foreground execution (required for `--json`). `[workflow.background].quiet` injects `--quiet` for detached jobs unless the caller set verbosity. Approve/merge/review prompt in the foreground when no non-interactive flag is provided and inject `--yes`/`--review-only` into the child after confirmation. If ask/draft input is read from stdin, Vizier runs in the foreground and rejects `--background`/`--follow`; use MESSAGE/`--file` when you need a detached run.
- Review checks: `[review.checks].commands = [ ... ]`; `vizier review` runs these unless `--skip-checks`, falling back to cargo check/test when unset in a Cargo repo.
- Merge behavior: `[merge].squash` (default true; `--squash`/`--no-squash`), `[merge].squash_mainline` (mainline parent for merge-heavy plan branches; `--squash-mainline <n>`), and `[merge.conflicts].auto_resolve` (default false; `--auto-resolve-conflicts`/`--no-auto-resolve-conflicts`).
- CI/CD gate: `[merge.cicd_gate]` controls `script` (default none), `auto_resolve` (default false; gate remediation toggle), and `retries` (default 1). CLI overrides: `--cicd-script`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, `--cicd-retries`. `vizier review` runs this gate once per review with auto-fix disabled; `vizier merge` enforces it before completing.
- Approve stop-condition: `[approve.stop_condition]` controls `script` (default none; repo-local shell script) and `retries` (default 3; maximum number of extra agent attempts after the first). When configured, `vizier approve` re-runs the agent on the draft branch until the script exits 0 or the retry budget is exhausted. CLI overrides: `vizier approve --stop-condition-script <PATH>` and `--stop-condition-retries <COUNT>`.

## Inspecting and selecting agents per command
- Each assistant command resolves to a single selector: `[agents.default]` seeds all scopes; per-scope tables override; CLI flags win. Misconfigured selectors/scripts cause the command to fail rather than falling back.
- Runtime resolution order: `[agents.<scope>.agent].command` (custom script) → bundled shim for the chosen selector. Progress filters default to the shim’s bundled filter when unset.
- `vizier plan --json` surfaces the resolved selector, shim/command path, prompt source, and documentation toggles per scope so you can confirm the effective settings before running.

## Help, pager, and display levers
- `--no-ansi` disables ANSI even on TTY; non-TTY always omits ANSI. Quiet (`-q`) suppresses progress history but still prints help/output when explicitly requested.
- Help paging honors `$VIZIER_PAGER` (defaults to `less -FRSX`), uses paging only on TTY by default, and can be forced/disabled via `--pager`/`--no-pager`.
- Progress is now purely line-based: `-v`/`-vv` lift stderr verbosity while `--json` prints outcome JSON to stdout where available.

## Repo plumbing and git hygiene
- Push control: `--push` pushes the current branch after mutating history; no config key exists.
- Merge history: defaults to squashing implementation commits before writing the merge commit; config/CLI flags above control mainline selection and conflict auto-resolve posture.
- Pending-commit posture: `.vizier` edits and narrative updates follow the pending-commit gate; use `--no-commit`/`[workflow].no_commit_default` to hold changes for inspection.

## Deprecated and rejected keys
- `backend = ...` entries still parse for compatibility but emit warnings; migrate to `agent = ...` and drop `agent.label` in favor of selectors plus explicit `[agent]` command overrides when needed.
- `fallback_backend`/`fallback-backend` and any `agent|backend="wire"` entries are rejected; fix configs instead of relying on silent fallback.
- Legacy prompt keys still work (`[prompts]`, `.vizier/BASE_SYSTEM_PROMPT.md`), but `[agents.<scope>.prompts.<kind>]` is the primary surface and should stay in sync with `docs/prompt-config-matrix.md`.
