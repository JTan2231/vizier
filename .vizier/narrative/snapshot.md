Running Snapshot — updated (2026-02-14)

Narrative theme
- Hard-remove workflow/agent command families: Vizier now exposes a reduced, stable CLI contract centered on repository initialization, pending-plan visibility, job record operations, completions, and release creation.
- Enforce strict removal semantics: deleted commands and hidden workflow entrypoints must fail through standard Clap unknown-subcommand behavior, with no custom migration guidance text in CLI errors.
- Keep operator surfaces coherent: help, man pages, install assets, docs, and tests all reflect the reduced command set and removed global workflow flags.
- Preserve historical artifacts: `vizier jobs` remains operational for existing on-disk job records, including approval/retry/cancel/tail/schedule views.
- Continue internal workflow-model cleanup without resurrecting removed commands: kernel/template internals now use executor-first classification (`environment.builtin`, `environment.shell`, `agent`) with control policy typed separately, canonicalize agent execution to `cap.agent.invoke`, and require explicit prompt producer/consumer wiring for canonical invoke nodes.
- Legacy plan artifacts from removed workflows still drift across worktrees (branch/doc mismatches), so archival hygiene remains part of reduced-surface stabilization.

Active threads
- Reduced CLI surface stabilization: ACTIVE. Ensure removed top-level commands (`save`, `draft`, `approve`, `review`, `merge`, `test-display`, `plan`, `build`, `patch`, `run`) and hidden workflow paths are absent from parser/help/man/docs/tests; removed globals (`--agent`, `--push`, `--no-commit`, `--follow`, `--background-job-id`) remain unsupported. [Cross: stdout/stderr contract, portable man docs]
- Init contract durability: ACTIVE. `vizier init` / `vizier init --check` remain the canonical bootstrap and validation path for durable marker files + required ignore rules.
- Jobs/read-only scheduler operations: ACTIVE. `vizier jobs` continues to expose list/schedule/show/status/tail/attach/approve/reject/retry/cancel/gc against persisted records.
- Release reliability: ACTIVE. `vizier release` remains intact with dry-run, bump overrides, tag controls, and release-note filtering.
- Executor-first workflow taxonomy: ACTIVE. `vizier-kernel` template validation now classifies nodes as executor vs control, requires explicit executor IDs, canonicalizes agent runtime execution to `cap.agent.invoke`, enforces prompt artifact contracts on canonical invoke/prompt-resolve nodes, rejects implicit unknown `uses` fallbacks, and emits compatibility warnings for legacy `cap.*`/`vizier.*` aliases through a dated migration window. [Cross: jobs metadata observability, scheduler docs]
- Workflow/agent orchestration threads: RETIRED. Prior draft/approve/review/merge, build/patch/run orchestration, backend-pluggability, and template-reduction expansion tracks are archived as historical context after hard removal.

Code state (behaviors that matter)
- Top-level CLI command set is now:
  - `help`
  - `init`
  - `list`
  - `cd`
  - `clean`
  - `jobs`
  - `completions`
  - `release`
- Removed commands (`save`, `draft`, `approve`, `review`, `merge`, `test-display`, `plan`, `build`, `patch`, `run`) and hidden workflow paths are no longer parsed; invocations fail as unrecognized subcommands through Clap.
- Removed global flags (`--agent`, `--push`, `--no-commit`, `--follow`, hidden `--background-job-id`) are no longer accepted. `jobs tail` now owns `--follow` locally.
- Dispatch and actions were reduced to retained surfaces; workflow action modules and workflow-template dispatch wiring were deleted from the CLI build.
- Man-page output now ships only `man1/vizier.1` + `man1/vizier-jobs.1` + `man5/vizier-config.5` + `man7/vizier-workflow.7`; `man1/vizier-build.1` is removed.
- Install/test assets now stage and validate the reduced man-page set.
- Integration coverage was resliced to retained commands and explicit negative coverage for removed-command unknown-subcommand behavior.
- Kernel workflow-template compilation now emits node classification metadata (`node_class`, `executor_class`, `executor_operation`, `control_policy`) and warning diagnostics for legacy capability aliases; compile/validate rejects unknown implicit `uses` labels instead of auto-mapping custom labels to executable capability.
- Canonical executor operation map now treats `cap.agent.invoke` as the single agent runtime primitive; legacy agent-purpose IDs (`cap.agent.plan.*`, `cap.agent.review.*`, `cap.agent.remediation.*`, `cap.agent.merge.resolve_conflict`) classify through compatibility aliases and normalize executor operation to `agent.invoke`.
- Canonical validator contracts now require:
  - `cap.agent.invoke`: `kind=agent`, exactly one prompt dependency (`custom:prompt_text:<key>`), and no inline `args.command`/`args.script`.
  - `cap.env.builtin.prompt.resolve`: `kind=builtin`, no inline command/script args, exactly one prompt artifact output.
  - `cap.env.shell.prompt.resolve`: `kind=shell|custom`, exactly one of `args.command`/`args.script`, exactly one prompt artifact output.
- Maintained repository template artifacts (`.vizier/workflow/{draft,approve,merge}.toml`, `.vizier/develop.toml`) now use explicit `prompt.resolve -> agent.invoke` chains with `prompt_text` artifact contracts (`v2`) while keeping command-surface behavior unchanged.
- Job metadata/read paths now accept and render executor identity fields (`workflow_executor_class`, `workflow_executor_operation`, `workflow_control_policy`) alongside legacy `workflow_capability_id` for historical records.
- Legacy `draft/*` branches and `.vizier/implementation-plans/*.md` files can still appear in non-bijective states in existing worktrees; treat them as historical residue, not an active command surface.
- Current worktree evidence (`draft/invoke`): `.vizier/implementation-plans/` has no on-disk plan docs, `.vizier/implementation-plans/invoke.md` is currently tracked as deleted, and local branches include `draft/after`, `draft/approve`, `draft/invoke`, and `draft/prompting-md` (plus `build/patch-740958c5f8bf` outside draft-plan mapping).

Acceptance checkpoints (selected)
- `vizier --help` / `vizier help --all` list only the retained command set and current global flags.
- Each removed command returns generic unknown-subcommand errors without custom migration guidance.
- Generated man pages and installation manifests contain no `vizier-build` page and no removed command inventory.
- Workflow-template validator coverage asserts executor/control classification, canonical prompt->invoke validation, legacy-alias diagnostics, and rejection of unknown implicit `uses` labels.
- `vizier jobs show` can surface canonical executor identity metadata (`agent.invoke`) for new records while preserving legacy workflow capability metadata fields.
- `cargo check --all --all-targets`, `cargo test --all --all-targets`, and `./cicd.sh` pass on this branch.

Next moves
1) Decide when to hard-enforce prompt-dependency and prompt-output contracts for legacy agent-purpose alias IDs (currently strict only for canonical `cap.agent.invoke` / `cap.env.*.prompt.resolve`).
2) Decide where/when scheduler enqueue paths should populate new executor identity metadata fields automatically (current change is additive read/display + schema support).
3) Keep pruning now-unreachable workflow/template internals while preserving compatibility for persisted job artifacts.
4) Decide whether deprecated `cd`/`clean` should remain as explicit erroring commands or be removed in a follow-up hard cut.
5) Define archive cleanup policy for legacy `draft/*` branch and `.vizier/implementation-plans/*.md` drift so retained plan-visibility surfaces stay interpretable after workflow removal.
