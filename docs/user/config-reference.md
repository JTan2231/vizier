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
- Pin review to Gemini while leaving other commands on Codex: set `[agents.commands.review] agent = "gemini"` (or legacy `[agents.review]`) and run `vizier plan --json` to confirm the resolved selector before `vizier review`. CLI override: `vizier review --agent gemini` for one-offs.
- Tighten merge CI/CD gates: set `[merge.cicd_gate] script = "./cicd.sh"`, `retries = 3`, and (optionally) `auto_resolve = true` so Vizier retries failures up to three times with agent remediation. Override per run with `--cicd-script`, `--cicd-retries`, and `--auto-cicd-fix/--no-auto-cicd-fix`.
- Disable auto-commit for inspection: set `[workflow] no_commit_default = true` to hold assistant edits dirty/staged across save/draft/approve/review. For a single run, pass `--no-commit`; re-run without it before merging so history is finalized.
- Scheduler execution: assistant-backed commands always enqueue background jobs. Use `--follow` to attach to the job’s stdout/stderr stream; `--json` is not supported (use `vizier jobs show --format json` instead). On a TTY, `vizier approve`/`vizier merge` prompt for confirmation before the job is queued, and `vizier review` prompts for the review mode unless you pass `--yes`/`--review-only`/`--review-file`. Non-TTY runs require explicit flags. `vizier approve --require-approval` adds an explicit human gate so a queued job pauses at `waiting_on_approval` until someone runs `vizier jobs approve <JOB_ID>` (or `vizier jobs reject <JOB_ID> --reason ...` blocks it). `[workflow.background].quiet` controls whether detached jobs inject `--quiet` when you do not pass verbosity flags.
- Scheduler sequencing: `vizier save|draft|approve|review|merge --after <JOB_ID>` adds an explicit predecessor constraint (repeatable). Every listed predecessor must finish with `succeeded` before the new job can start.
- Scheduler recovery: `vizier jobs retry <JOB_ID>` rewinds the requested failed/blocked job and its downstream dependents, then re-queues them in one step. Retry refuses when any job in that retry set is still `running`; queued/waiting dependents are rewound automatically. Retry uses best-effort owned-worktree cleanup (libgit2 prune first, then `git worktree remove --force` + `git worktree prune --expire now` fallback on prune failures) and keeps cleanup pointers when cleanup degrades.
- Swap prompt text for a single command alias: add `[agents.commands.merge.prompts.merge_conflict] path = ".vizier/MERGE_CONFLICT_PROMPT.md"` (or `text = """..."""`) to override just merge-conflict prompting without touching other commands.
- Pin wrapper-to-template mapping for audits: set `[commands]` entries (for example `build_execute = "template.build_execute@v1"`) and confirm via `vizier plan --json` before queueing jobs.
- Load a wrapper template from disk: set `[commands].merge = "file:.vizier/workflow/merge.json"` (or a direct path like `./.vizier/workflow/merge.toml`) to use a repo-managed template file. Legacy `[workflow.templates].merge` is still read as fallback during migration.

## Override matrix (config vs CLI)
- Agent selector/runtime: `[agents.commands.<alias>] agent` picks the shim (`codex`, `gemini`, or a custom name) and `[agents.commands.<alias>.agent]` customizes runtime command/filter/output. `[agents.templates."<id@version>"]` can override per template selector. Legacy `[agents.<scope>]` is still accepted during migration. CLI `--agent`, `--agent-command`, `--agent-label` override everything for the current run.
- Prompt selection: `[agents.commands.<alias>.prompts.<kind>]` and `[agents.templates."<id@version>".prompts.<kind>]` (`text`/`path` + nested `[agent]` overrides) → legacy `[agents.<scope>.prompts.<kind>]` (compatibility) → `[agents.default.prompts.<kind>]` → `.vizier/*PROMPT*.md` → baked defaults.
- Workflow hold: `[workflow].no_commit_default` (default false) ↔ CLI `--no-commit` flag.
- Wrapper template mapping: `[commands].{save,draft,approve,review,merge,build_execute,patch}` is the primary selector map. Legacy `[workflow.templates]` is still read as fallback during migration. Supported selectors are `id@version`, `.vN` suffix forms, and file-backed selectors (`file:<path>` or direct path-like values such as `./template.json`/`./template.toml`). No CLI override today; inspect with `vizier plan`.
- Scheduler posture: assistant-backed commands always enqueue jobs; use `--follow` to attach to logs. On a TTY, approve/merge prompt for confirmation before queueing and review prompts for mode selection unless you provided a review flag; in non-TTY contexts you must pass those flags explicitly. `[workflow.background].quiet` (default false) injects `--quiet` for detached jobs unless the caller set verbosity flags.
- Scheduler ordering: `--after <JOB_ID>` is repeatable on scheduler-backed commands and has no config default; dependencies are explicit per run.
- Scheduler retry: `vizier jobs retry <JOB_ID>` currently has no config knobs; it always rewinds from the requested job root with scheduler-owned best-effort cleanup. Retry records `retry_cleanup_status`/`retry_cleanup_error` in job metadata and only clears `worktree_*` metadata when cleanup is done/skipped.
- Job cancel cleanup: `[jobs.cancel].cleanup_worktree` (default false) opts in to deleting job-owned worktrees when you run `vizier jobs cancel`; override per run with `vizier jobs cancel --cleanup-worktree` or force off with `--no-cleanup-worktree`. Cleanup runs only on operator-initiated cancellation, not on job failures.
- Merge gates: `[merge.cicd_gate].{script,retries,auto_resolve}` ↔ CLI `--cicd-script`, `--cicd-retries`, `--auto-cicd-fix/--no-auto-cicd-fix`.
- Merge history: `[merge].squash` / `[merge].squash_mainline` ↔ CLI `--squash`/`--no-squash`, `--squash-mainline`.
- List output: `[display.lists.*].format`/`fields` ↔ CLI `vizier list --format/--fields`, `vizier jobs list --format`, `vizier jobs show --format`; global `--json` overrides formats.
- Display/help: pager defaults are TTY-only; use `--pager`/`--no-pager` or `$VIZIER_PAGER` to force/disable. `--no-ansi` strips color; `-q/-v/-vv` control verbosity.
- Checks/review: `[review.checks].commands` ↔ CLI `vizier review --skip-checks` (to skip) or config to set the commands; merge CI/CD gate is reused during review with auto-fix disabled.

## Build orchestration settings
- Build defaults: `[build]` controls execute-time orchestration defaults (`default_pipeline`, `default_merge_target`, `default_review_mode`, `default_skip_checks`, `default_keep_draft_branch`).
- Patch defaults: `vizier patch` uses `approve-review-merge` when `--pipeline` is omitted (independent of `[build].default_pipeline`); pass `vizier patch --pipeline ...` to override per run.
- Graph controls: `[build].stage_barrier` (`strict|explicit`) and `[build].failure_mode` (`block_downstream|continue_independent`) tune dependency behavior/audit posture for `vizier build execute`.
- Profile presets: `[build].default_profile` and `[build.profiles.<name>]` let you reuse policy bundles (`pipeline`, `merge_target`, `review_mode`, `skip_checks`, `keep_branch`) across steps.
- CLI override: `vizier build execute --pipeline ...` overrides per-step/profile/config pipeline selection for that run.
- Step-level build-file overrides (`profile`, `pipeline`, `merge_target`, `review_mode`, `skip_checks`, `keep_branch`, `after_steps`) remain authoritative for fields without CLI flags.

## Agents, prompts, and documentation toggles
- `agent` (root or `[agents.default]`): selector for the bundled shim (`codex` by default, `gemini` as the alternate) or any custom shim name you’ve installed.
- Legacy `backend` / `fallback_backend` keys are rejected; migrate to `agent` selectors and remove fallback entries.
- Agent runtime overrides: `[agents.commands.<alias>.agent]` and `[agents.templates."<selector>".agent]` provide `command` (custom script), optional `progress_filter`, `output` (`auto`/wrapped JSON), and `enable_script_wrapper`. Legacy `[agents.<scope>.agent]` remains as fallback during migration.
- Per-command selector overrides: `[agents.commands.save|draft|approve|review|merge|patch|build_execute] agent="<selector>"`; CLI `--agent/--agent-label/--agent-command` override all command/template tables for the current run.
- Internal fallback profile: non-command prompt/session fallback paths resolve through `[agents.default]` and may emit `scope = "default"` in session metadata; this `default` profile is internal and is not a CLI command scope.
- Documentation prompt toggles: `[agents.commands.<alias>.documentation]` and `[agents.templates."<selector>".documentation]` with `enabled` (default true), `include_snapshot` (default true), and `include_narrative_docs` (default true). Legacy `[agents.<scope>.documentation]` remains as fallback during migration.
- Prompt text and per-prompt agent overrides: `[agents.commands.<alias>.prompts.<kind>]` and `[agents.templates."<selector>".prompts.<kind>]` set `text`/`path` plus nested `[agent]` runtime overrides for that identity. Legacy `[agents.<scope>.prompts.<kind>]` remains as fallback during migration. Prompt kinds are strict: `documentation`, `commit`, `implementation_plan`, `review`, `merge_conflict`.
- Progress filters attach by selector: when `progress_filter` is unset, Vizier looks for a bundled `filter.sh` under the configured selector (Codex, Gemini, or any custom shim with a sibling filter) and wires it automatically.

## Workflow and gate settings
- Workflow defaults: `[workflow].no_commit_default` (default false) pairs with `--no-commit` to hold assistant edits for manual review across save/draft/approve/review.
- Workflow templates: `[commands]` is the primary wrapper-alias mapping (`save`, `draft`, `approve`, `review`, `merge`, `patch`, `build_execute`), and legacy `[workflow.templates]` remains compatibility fallback during migration. Built-in defaults resolve to `template.<alias>.v1`.
- File-backed workflow templates: when a selector resolves to a local JSON/TOML template file, Vizier loads it at runtime and interpolates `${param}` markers from template/default/runtime params before compile.
- Capability catalog: `CAPABILITIES.md` documents the canonical capability ids and compatibility label aliases used by workflow dispatch.
- Workflow template compilation rejects undeclared artifact contracts, invalid artifact-contract schemas, schema-invalid artifact payloads, and unknown `after` node references before a job is queued.
- Outcome fan-out is supported: `on.succeeded|failed|blocked|cancelled` may target multiple nodes, and Vizier normalizes/deduplicates target IDs at compile time.
- Custom template extension points: templates may declare custom node kinds (`kind = "custom"`) and custom artifacts (`{ "custom": { "type_id": "...", "key": "..." } }`) as long as every custom artifact `type_id` has a declared artifact contract id.
- Wrapper aliases (`save`, `draft`, `approve`, `review`, `merge`, `patch`) always enqueue the full compiled template DAG in topological `after` order (built-in and file/custom selectors alike). The queued root job id binds to the semantic primary node for stable UX and `--follow` behavior.
- Wrapper primary-node binding is semantic and capability-first: Vizier first looks for the canonical built-in node id (for backward compatibility), then falls back to a unique node that resolves to the expected capability (for example `cap.plan.apply_once`), so custom templates can rename node ids and keep alias labels without breaking wrappers.
- `vizier build execute` schedules every compiled build-execute template node (not fixed phase IDs). All nodes run through hidden `__workflow-node` jobs; canonical capabilities (`cap.build.materialize_step`, `cap.plan.apply_once`, `cap.review.critique_or_fix`, `cap.git.integrate_plan_branch`) use node-runtime built-in executors, while generic custom capability nodes use generic node execution.
- Custom artifacts now persist scheduler markers under `.vizier/jobs/artifacts/custom/...`; dependency checks probe these markers, and `vizier jobs retry` removes markers owned by rewound jobs before requeue.
- Build-execute resume state is keyed by template `policy.resume.key` (`execution.<key>.json`, with `default -> execution.json`), and `policy.resume.reuse_mode` controls drift behavior (`strict` rejects any drift, `compatible` permits policy-only drift).
- Wrapper gate settings flow into compiled template metadata: scheduled `approve` records stop-condition gates, and scheduled `review`/`merge` record effective CI/CD gate settings (including CLI overrides) in `workflow_gates`. Workflow jobs also record the resolved capability id as `workflow_capability`.
- Built-in `template.approve@v1` and `template.merge@v1` enqueue their control nodes (`approve_gate_stop_condition`, `merge_conflict_resolution`, `merge_gate_cicd`, `merge_cicd_auto_fix`) as explicit scheduler jobs. These built-in control jobs currently run in compatibility/no-op mode while the primary node continues to enforce gate/retry behavior for parity.
- Runtime gate execution for `approve` stop-condition retries, `review` CI/CD probes, and `merge` CI/CD remediation now resolves from compiled template node gate/retry policy (not ad hoc command-local settings).
- Merge conflict auto-resolution is also template-derived: by default `merge_integrate.on.blocked -> merge_conflict_resolution`, but custom node IDs work when semantic capabilities (`cap.git.integrate_plan_branch`, `cap.gate.conflict_resolution`) are preserved (label aliases are fine).
- Background jobs: assistant-backed commands always enqueue background runs; `--follow` tails logs and `--json` is not supported for these commands. On a TTY, approve/merge prompt for confirmation before queueing and review prompts for mode selection unless you passed `--yes`/`--review-only`/`--review-file`; non-TTY runs require explicit flags. `[workflow.background].quiet` (default false) injects `--quiet` for detached jobs unless the caller set verbosity. Detached jobs flush stdout/stderr before marking the job complete so `vizier jobs tail --follow` captures the final assistant output. Use `vizier jobs list --dismiss-failures` to hide failed entries unless `--all` is set. Job outcome JSON includes `schedule.waited_on` when a run was delayed by dependencies, approval, locks, or pinned-head mismatches.
- Job timeout: agent-backed jobs abort after 12 hours by default. The only CLI override today is `vizier test-display --timeout <SECONDS>` for smoke tests; there is no config key yet.
- Job cancel cleanup: `[jobs.cancel].cleanup_worktree` (default false) controls whether `vizier jobs cancel` removes the job-owned worktree. Cleanup never runs on job failures; use `--cleanup-worktree` or `--no-cleanup-worktree` to override per cancel.
- Review checks: `[review.checks].commands = [ ... ]`; `vizier review` runs these unless `--skip-checks`, falling back to cargo check/test when unset in a Cargo repo.
- Merge behavior: `[merge].squash` (default true; `--squash`/`--no-squash`), `[merge].squash_mainline` (mainline parent for merge-heavy plan branches; `--squash-mainline <n>`), and `[merge.conflicts].auto_resolve` (default false; `--auto-resolve-conflicts`/`--no-auto-resolve-conflicts`).
- CI/CD gate: `[merge.cicd_gate]` controls `script` (default none), `auto_resolve` (default false; gate remediation toggle), and `retries` (default 1). CLI overrides: `--cicd-script`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, `--cicd-retries`. `vizier review` runs this gate once per review with auto-fix disabled using compiled template gate policy; `vizier merge` evaluates the gate and retries remediation using template-derived `cicd + until_gate` policy.
- Approve stop-condition: `[approve.stop_condition]` controls `script` (default none; repo-local shell script) and `retries` (default 3; maximum number of extra agent attempts after the first). When configured, `vizier approve` re-runs plan application until the script exits 0 or the template-derived retry budget is exhausted. CLI overrides: `vizier approve --stop-condition-script <PATH>` and `--stop-condition-retries <COUNT>`.

## Commit metadata and merge templates
- Commit metadata injection: `[commits.meta]` controls whether session IDs, session logs, author notes, and narrative summaries are injected into commit messages. Defaults are enabled, `style = "header"`, `include = ["session_id","session_log","author_note","narrative_summary"]`, and `session_log_path = "relative"` (values: `relative|absolute|none`). Set `style = "trailers"` to move metadata to the end of the message, `both` to duplicate, or `none`/`enabled = false` to omit metadata entirely. Allowed `include` values: `session_id`, `session_log`, `author_note`, `narrative_summary`.
- Commit metadata labels: `[commits.meta.labels]` overrides the label text used for metadata lines (`session_id`, `session_log`, `author_note`, `narrative_summary`).
- Fallback subjects: `[commits.fallback_subjects]` sets the subject line used when a commit summary is empty (`code_change`, `narrative_change`, `conversation`).
- Implementation commit template: `[commits.implementation]` controls the squash implementation commit subject (supports `{slug}`) and which fields appear in the body (`Target branch`, `Plan branch`, `Summary`).
- Merge commit template: `[commits.merge]` controls the merge commit subject (supports `{slug}`), whether operator notes are included, the operator note label, and plan embedding (`plan_mode = full|summary|none`, `plan_label`).

## List output formatting
- `vizier list`: `[display.lists.list]` controls `format` (`block|table|json`), header/entry/job/command field ordering, summary truncation (`summary_max_len` default 120, `summary_single_line` default true), and label overrides (`labels`).
- `vizier jobs list`: `[display.lists.jobs]` controls `format`, whether succeeded jobs are shown (`show_succeeded`), field ordering, and label overrides. Built-in fields include `Job`, `Status`, `Created`, `After`, `Wait`, `Approval required`, `Approval state`, `Approval decided by`, `Dependencies`, `Locks`, `Pinned head`, `Failed`, and `Command`. CLI `--all` overrides `show_succeeded`.
- `vizier jobs show`: `[display.lists.jobs_show]` controls `format`, field ordering, and label overrides, including the `After` field (`<job-id> (success)` entries).
- `vizier jobs schedule`: output is command-scoped (not config-driven) with `--format summary|dag|json`; default is human-readable `summary` (one row per visible job with deterministic `created_at` then `job_id` ordering), `--format dag` preserves recursive dependency traversal, and `--format json` (or global `--json`) emits a stable contract with `{version, ordering, jobs[], edges[]}` where `version=1` and `ordering="created_at_then_job_id"`.
- Build execution metadata for `jobs show`: include `Build pipeline`, `Build target`, `Build review mode`, `Build skip checks`, `Build keep branch`, and `Build dependencies` in `[display.lists.jobs_show].fields` (or rely on defaults) to audit effective per-step policy from job records.
- Workflow-template metadata for `jobs show`: include `Workflow template`, `Workflow template version`, `Workflow node`, `Workflow capability`, `Workflow policy snapshot`, and `Workflow gates` to inspect declarative compile context per job.
- Patch execution metadata for `jobs show`: include `Patch file`, `Patch index`, and `Patch total` to trace ordered `vizier patch` runs across per-phase jobs.
- `vizier jobs status`: prints a terse one-line status; `--json` emits `job`, `status`, `exit_code`, `stdout`, and `stderr` fields.
- `vizier jobs retry`: rewinds a failed/blocked job chain and prints a block outcome with `Requested`, `Retry root`, `Last successful point`, `Retry set`, `Reset`, `Restarted`, and optional `Updated`; `--json` emits the same fields as structured arrays/strings.
- `vizier jobs approve` / `vizier jobs reject`: resolve explicit scheduler approval gates; JSON output includes the resulting `status` and `approval_state`.
- CLI overrides: `vizier list --format`, `vizier list --fields` override the list display settings; `vizier jobs list --format` and `vizier jobs show --format` override job display formats. The global `--json` flag forces JSON output for list-style commands regardless of config.

## Inspecting and selecting agents per command
- Each assistant command resolves to a single selector: `[agents.default]` seeds all commands; template tables override alias tables; legacy scope tables are compatibility fallback only; CLI flags win.
- Runtime resolution order: template table (`[agents.templates."<selector>"]`) → alias table (`[agents.commands.<alias>]`) → legacy scope (`[agents.<scope>]`) → default.
- `vizier plan --json` surfaces the resolved selector, shim/command path, template selector, prompt source, and documentation toggles per command alias.

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
