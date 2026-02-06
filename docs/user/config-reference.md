# Vizier configuration reference

This file is the authoritative catalogue of Vizier’s configuration levers, their defaults, and how CLI flags override them. Pair it with `vizier plan` (or `vizier plan --json`) to inspect the fully resolved configuration for your current repo + global settings + CLI overrides.

## How configuration is loaded
- CLI flags have the last word; use them for one-off overrides.
- Without `--config-file`, Vizier overlays `~/.config/vizier/config.toml` (or platform equivalent) with `.vizier/config.toml`/`.json` in the repo; missing keys inherit from the lower layer.
- `--config-file <path>` (or `--config-file=<path>`) replaces the search. If no config files are found, Vizier falls back to `$VIZIER_CONFIG_FILE` when it points at an existing file.
- `VIZIER_CONFIG_DIR`/`XDG_CONFIG_HOME`/`APPDATA`/`HOME`/`USERPROFILE` influence the global config location; `VIZIER_AGENT_SHIMS_DIR` can point at bundled agent shims when you relocate them.
- The CLI/frontends resolve config sources and precedence; the kernel only validates and normalizes the resolved config (it does not read files directly).
- Use `vizier plan --json` to see the merged config (per-command agent selection, prompt profiles, gate settings) before running draft/approve/review/merge.

## Quick-start scenarios
- Pin review to Gemini while leaving other scopes on Codex: set `[agents.review] agent = "gemini"` (optionally override `[agents.review.agent].command` if you moved the shim) and run `vizier plan --json` to confirm the resolved selector before `vizier review`. CLI override: `vizier review --agent gemini` for one-offs.
- Tighten merge CI/CD gates: set `[merge.cicd_gate] script = "./cicd.sh"`, `retries = 3`, and (optionally) `auto_resolve = true` so Vizier retries failures up to three times with agent remediation. Override per run with `--cicd-script`, `--cicd-retries`, and `--auto-cicd-fix/--no-auto-cicd-fix`.
- Disable auto-commit for inspection: set `[workflow] no_commit_default = true` to hold assistant edits dirty/staged across ask/save/draft/approve/review. For a single run, pass `--no-commit`; re-run without it before merging so history is finalized.
- Scheduler execution: assistant-backed commands always enqueue background jobs. Use `--follow` to attach to the job’s stdout/stderr stream; `--json` is not supported (use `vizier jobs show --format json` instead). On a TTY, `vizier approve`/`vizier merge` prompt for confirmation before the job is queued, and `vizier review` prompts for the review mode unless you pass `--yes`/`--review-only`/`--review-file`. Non-TTY runs require explicit flags. `[workflow.background].quiet` controls whether detached jobs inject `--quiet` when you do not pass verbosity flags.
- Scheduler sequencing: `vizier ask|save|draft|approve|review|merge --after <JOB_ID>` adds an explicit predecessor constraint (repeatable). Every listed predecessor must finish with `succeeded` before the new job can start.
- Swap prompt text for a single scope: add `[agents.merge.prompts.merge_conflict] path = ".vizier/MERGE_CONFLICT_PROMPT.md"` (or `text = """..."""`) to override just merge-conflict prompting without touching other commands.

## Override matrix (config vs CLI)
- Agent selector/runtime: `[agents.<scope>] agent` picks the shim (`codex`, `gemini`, or a custom name) and `[agents.<scope>.agent]` customizes the runtime command/filter/output; CLI `--agent`, `--agent-command`, `--agent-label` override all scopes for the current run.
- Prompt selection: `[agents.<scope>.prompts.<kind>]` (`text`/`path` + nested `[agent]` overrides) → `.vizier/*PROMPT*.md` → baked defaults; no CLI flag exists. Inspect with `vizier plan --json`.
- Workflow hold: `[workflow].no_commit_default` (default false) ↔ CLI `--no-commit` flag.
- Scheduler posture: assistant-backed commands always enqueue jobs; use `--follow` to attach to logs. On a TTY, approve/merge prompt for confirmation before queueing and review prompts for mode selection unless you provided a review flag; in non-TTY contexts you must pass those flags explicitly. `[workflow.background].quiet` (default false) injects `--quiet` for detached jobs unless the caller set verbosity flags.
- Scheduler ordering: `--after <JOB_ID>` is repeatable on scheduler-backed commands and has no config default; dependencies are explicit per run.
- Job cancel cleanup: `[jobs.cancel].cleanup_worktree` (default false) opts in to deleting job-owned worktrees when you run `vizier jobs cancel`; override per run with `vizier jobs cancel --cleanup-worktree` or force off with `--no-cleanup-worktree`. Cleanup runs only on operator-initiated cancellation, not on job failures.
- Merge gates: `[merge.cicd_gate].{script,retries,auto_resolve}` ↔ CLI `--cicd-script`, `--cicd-retries`, `--auto-cicd-fix/--no-auto-cicd-fix`.
- Merge history: `[merge].squash` / `[merge].squash_mainline` ↔ CLI `--squash`/`--no-squash`, `--squash-mainline`.
- List output: `[display.lists.*].format`/`fields` ↔ CLI `vizier list --format/--fields`, `vizier jobs list --format`, `vizier jobs show --format`; global `--json` overrides formats.
- Display/help: pager defaults are TTY-only; use `--pager`/`--no-pager` or `$VIZIER_PAGER` to force/disable. `--no-ansi` strips color; `-q/-v/-vv` control verbosity.
- Checks/review: `[review.checks].commands` ↔ CLI `vizier review --skip-checks` (to skip) or config to set the commands; merge CI/CD gate is reused during review with auto-fix disabled.

## Agents, prompts, and documentation toggles
- `agent` (root or `[agents.default]`): selector for the bundled shim (`codex` by default, `gemini` as the alternate) or any custom shim name you’ve installed.
- Legacy `backend` / `fallback_backend` keys are rejected; migrate to `agent` selectors and remove fallback entries.
- Agent runtime overrides: `[agents.<scope>.agent]` provide `command` (custom script), optional `progress_filter`, `output` (`auto`/wrapped JSON), and `enable_script_wrapper` (wraps non-shim scripts). Defaults are inferred from the selector; drop down to this table only when you need to point at a non-bundled script.
- Per-scope selector overrides: `[agents.ask|save|draft|approve|review|merge] agent="<selector>"`; CLI `--agent/--agent-label/--agent-command` override all scopes for the current run.
- Documentation prompt toggles: `[agents.<scope>.documentation]` with `enabled` (default true), `include_snapshot` (default true), and `include_narrative_docs` (default true). Disable or trim context for conflict auto-resolve or other low-context flows. Narrative context now comes exclusively from `.vizier/narrative/` (including `.vizier/narrative/glossary.md` and thread docs); legacy `.vizier/todo_*.md` files are no longer read.
- Prompt text and per-prompt agent overrides: `[agents.<scope>.prompts.<kind>]` sets `text`/`path` plus nested `[agent]` runtime overrides for that scope+kind; fall back to `.vizier/*.md` prompt files, then baked-in defaults. Prompt configuration is read only from `[agents.<scope>.prompts.<kind>]` (or `[agents.default.prompts.<kind>]`); `[prompts]` and `[prompts.<scope>]` are ignored, and `.vizier/BASE_SYSTEM_PROMPT.md` is not read. Prompt kinds are strict: `documentation`, `commit`, `implementation_plan`, `review`, `merge_conflict`; aliases like `base`, `system`, `plan`, `refine`, `merge` are rejected (see `docs/user/prompt-config-matrix.md` for the scope×kind map).
- Progress filters attach by selector: when `progress_filter` is unset, Vizier looks for a bundled `filter.sh` under the configured selector (Codex, Gemini, or any custom shim with a sibling filter) and wires it automatically.

## Workflow and gate settings
- Workflow defaults: `[workflow].no_commit_default` (default false) pairs with `--no-commit` to hold assistant edits for manual review across ask/save/draft/approve/review.
- Background jobs: assistant-backed commands always enqueue background runs; `--follow` tails logs and `--json` is not supported for these commands. On a TTY, approve/merge prompt for confirmation before queueing and review prompts for mode selection unless you passed `--yes`/`--review-only`/`--review-file`; non-TTY runs require explicit flags. `[workflow.background].quiet` (default false) injects `--quiet` for detached jobs unless the caller set verbosity. Detached jobs flush stdout/stderr before marking the job complete so `vizier jobs tail --follow` captures the final assistant output. Use `vizier jobs list --dismiss-failures` to hide failed entries unless `--all` is set. Job outcome JSON includes `schedule.waited_on` when a run was delayed by dependencies, locks, or pinned-head mismatches.
- Job timeout: agent-backed jobs abort after 12 hours by default. The only CLI override today is `vizier test-display --timeout <SECONDS>` for smoke tests; there is no config key yet.
- Job cancel cleanup: `[jobs.cancel].cleanup_worktree` (default false) controls whether `vizier jobs cancel` removes the job-owned worktree. Cleanup never runs on job failures; use `--cleanup-worktree` or `--no-cleanup-worktree` to override per cancel.
- Review checks: `[review.checks].commands = [ ... ]`; `vizier review` runs these unless `--skip-checks`, falling back to cargo check/test when unset in a Cargo repo.
- Merge behavior: `[merge].squash` (default true; `--squash`/`--no-squash`), `[merge].squash_mainline` (mainline parent for merge-heavy plan branches; `--squash-mainline <n>`), and `[merge.conflicts].auto_resolve` (default false; `--auto-resolve-conflicts`/`--no-auto-resolve-conflicts`).
- CI/CD gate: `[merge.cicd_gate]` controls `script` (default none), `auto_resolve` (default false; gate remediation toggle), and `retries` (default 1). CLI overrides: `--cicd-script`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, `--cicd-retries`. `vizier review` runs this gate once per review with auto-fix disabled; `vizier merge` enforces it before completing.
- Approve stop-condition: `[approve.stop_condition]` controls `script` (default none; repo-local shell script) and `retries` (default 3; maximum number of extra agent attempts after the first). When configured, `vizier approve` re-runs the agent on the draft branch until the script exits 0 or the retry budget is exhausted. CLI overrides: `vizier approve --stop-condition-script <PATH>` and `--stop-condition-retries <COUNT>`.

## Commit metadata and merge templates
- Commit metadata injection: `[commits.meta]` controls whether session IDs, session logs, author notes, and narrative summaries are injected into commit messages. Defaults are enabled, `style = "header"`, `include = ["session_id","session_log","author_note","narrative_summary"]`, and `session_log_path = "relative"` (values: `relative|absolute|none`). Set `style = "trailers"` to move metadata to the end of the message, `both` to duplicate, or `none`/`enabled = false` to omit metadata entirely. Allowed `include` values: `session_id`, `session_log`, `author_note`, `narrative_summary`.
- Commit metadata labels: `[commits.meta.labels]` overrides the label text used for metadata lines (`session_id`, `session_log`, `author_note`, `narrative_summary`).
- Fallback subjects: `[commits.fallback_subjects]` sets the subject line used when a commit summary is empty (`code_change`, `narrative_change`, `conversation`).
- Implementation commit template: `[commits.implementation]` controls the squash implementation commit subject (supports `{slug}`) and which fields appear in the body (`Target branch`, `Plan branch`, `Summary`).
- Merge commit template: `[commits.merge]` controls the merge commit subject (supports `{slug}`), whether operator notes are included, the operator note label, and plan embedding (`plan_mode = full|summary|none`, `plan_label`).

## List output formatting
- `vizier list`: `[display.lists.list]` controls `format` (`block|table|json`), header/entry/job/command field ordering, summary truncation (`summary_max_len` default 120, `summary_single_line` default true), and label overrides (`labels`).
- `vizier jobs list`: `[display.lists.jobs]` controls `format`, whether succeeded jobs are shown (`show_succeeded`), field ordering, and label overrides. Built-in fields include `Job`, `Status`, `Created`, `After`, `Wait`, `Dependencies`, `Locks`, `Pinned head`, `Failed`, and `Command`. CLI `--all` overrides `show_succeeded`.
- `vizier jobs show`: `[display.lists.jobs_show]` controls `format`, field ordering, and label overrides, including the `After` field (`<job-id> (success)` entries).
- `vizier jobs status`: prints a terse one-line status; `--json` emits `job`, `status`, `exit_code`, `stdout`, and `stderr` fields.
- CLI overrides: `vizier list --format`, `vizier list --fields` override the list display settings; `vizier jobs list --format` and `vizier jobs show --format` override job display formats. The global `--json` flag forces JSON output for list-style commands regardless of config.

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

## Unsupported keys
Only the keys documented above are supported. Keys outside this surface are ignored or rejected when explicitly detected.
