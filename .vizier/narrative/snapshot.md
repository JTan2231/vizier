Running Snapshot — updated (2026-02-15)

Narrative theme
- Hard-remove wrapper workflow/agent command families while restoring one orchestrator front-door: Vizier keeps `save`/`draft`/`approve`/`review`/`merge`/`build`/`patch` removed and now exposes `run` for template-driven scheduling only.
- Enforce strict removal semantics: deleted commands and legacy hidden workflow entrypoints fail through standard Clap unknown-subcommand behavior, with no custom migration guidance text in CLI errors.
- Keep operator surfaces coherent: help, man pages, install assets, docs, and tests all reflect the reduced command set and removed global workflow flags.
- Keep help paging semantics explicit: help output is TTY-auto paged via `$VIZIER_PAGER` (or fallback pager) with hidden `--no-pager` suppression, while explicit `--pager` remains unsupported and should not be documented as a user-facing flag.
- Preserve historical artifacts: `vizier jobs` remains operational for existing on-disk job records, including approval/retry/cancel/tail/schedule views.
- Continue internal workflow-model cleanup without resurrecting removed commands: kernel/template internals now use executor-first classification (`environment.builtin`, `environment.shell`, `agent`) with control policy typed separately, canonicalize agent execution to `cap.agent.invoke`, and require explicit prompt producer/consumer wiring for canonical invoke nodes.
- Keep runtime plumbing unified under the hidden bridge: canonical template nodes materialize to scheduler jobs, run through `__workflow-node`, and persist prompt payloads in typed artifact data files; `vizier run` is now the public queue-time entrypoint.
- Land primitive stage-template cutover on the `run` front door: repo-local `draft`/`approve`/`merge` templates now execute as canonical primitive DAGs selected through `[commands]` aliases, with stage operations handled via `vizier jobs`.
- Keep global workflow config explicit: `[workflow.global_workflows]` now only governs out-of-repo allowlisting for explicit file selectors; `vizier run <flow>` no longer performs implicit repo/global alias discovery.
- Complete canonical runtime handler coverage behind the hidden bridge: all 16 executor operations and 5 control policies accepted by template validation now execute concretely (including real `agent.invoke` runner wiring, worktree lifecycle, plan/git/patch/build/sentinel ops, and conflict/cicd/approval/terminal policy handling).
- Propagate workflow execution roots deterministically across runtime edges: node metadata now carries `execution_root`, success edges propagate context to downstream queued nodes, retry-edge rewinds can inject propagated context before scheduler requeue, and cleanup/reset paths return context to repo root without changing topology semantics.
- Legacy plan artifacts from removed workflows still drift across worktrees (branch/doc mismatches), so archival hygiene remains part of reduced-surface stabilization.

Active threads
- Reduced CLI surface stabilization: ACTIVE. Keep wrapper removals intact (`save`, `draft`, `approve`, `review`, `merge`, `test-display`, `plan`, `build`, `patch`) while treating `run` as the only restored orchestration command. Removed globals (`--agent`, `--push`, `--no-commit`, `--follow`, `--pager`, `--background-job-id`) remain unsupported while hidden `--no-pager` stays internal-only. Internal runtime entrypoint `__workflow-node` stays hidden and scheduler-only. [Cross: stdout/stderr contract, portable man docs]
- Init contract durability: ACTIVE. `vizier init` / `vizier init --check` remain the canonical bootstrap and validation path for durable marker files + required ignore rules.
- Jobs/read-only scheduler operations: ACTIVE. `vizier jobs` continues to expose list/schedule/show/status/tail/attach/approve/reject/retry/cancel/gc against persisted records.
- Release reliability: ACTIVE. `vizier release` remains intact with dry-run, bump overrides, tag controls, and release-note filtering.
- Executor-first workflow taxonomy: ACTIVE. `vizier-kernel` template validation now classifies nodes as executor vs control, requires explicit canonical `uses` IDs, canonicalizes agent runtime execution to `cap.agent.invoke`, enforces prompt artifact contracts on canonical invoke/prompt-resolve nodes, and hard-rejects legacy `vizier.*` plus legacy non-env `cap.*` labels. Queue-time runtime compilation now materializes one hidden scheduler job per node with canonical workflow metadata and run-manifest wiring, and runtime dispatch now executes the full canonical operation/policy inventory with no placeholder fallthrough for canonical IDs. [Cross: jobs metadata observability, scheduler docs]
- Workflow-template reduction surface: ACTIVE. Stage orchestration now runs through `[commands]` aliases (`draft`, `approve`, `merge`, optional composed aliases like `develop`) that map to repo-local workflow files, with canonical stage DAG contracts, queue-time validation, and scheduler-only runtime controls (`vizier run` + `vizier jobs`). [Cross: configuration posture, scheduler docs]
- Workflow/agent orchestration threads: RETIRED. Prior draft/approve/review/merge, build/patch/run orchestration, backend-pluggability, and template-reduction expansion tracks are archived as historical context after hard removal.

Code state (behaviors that matter)
- Top-level CLI command set is now:
  - `help`
  - `init`
  - `list`
  - `cd`
  - `clean`
  - `jobs`
  - `run`
  - `completions`
  - `release`
- Removed commands (`save`, `draft`, `approve`, `review`, `merge`, `test-display`, `plan`, `build`, `patch`) are no longer parsed; invocations fail as unrecognized subcommands through Clap. Internal scheduler-only `__workflow-node` is parsed as a hidden command and is excluded from help/man/completion surfaces.
- Removed global flags (`--agent`, `--push`, `--no-commit`, `--follow`, `--pager`, hidden `--background-job-id`) are no longer accepted. Help paging is automatic on TTY with `$VIZIER_PAGER`/fallback pager, while hidden `--no-pager` remains an internal suppression hook. `jobs tail` now owns `--follow` locally.
- `vizier run` now resolves flow sources in deterministic order (explicit file/path, `[commands]` alias selector, canonical selector lookup), loads canonical/composed templates (`imports` + `links`), applies `${key}` parameter expansion from template defaults plus `--set`, and hard-fails queue-time on unresolved placeholders/compose cycles/validator errors before any enqueue.
- Explicit template file selectors now stay repo-bounded by default but may resolve outside the repo when the file lives under the configured global workflow directory.
- Workflow config now includes `[workflow.global_workflows]` (`enabled = true` by default, optional `dir` override where empty means platform default `<base_config_dir>/vizier/workflows`).
- Repo stage alias mapping now lives in `.vizier/config.toml` under `[commands]` (`draft`, `approve`, `merge`, and `develop`) and routes stage orchestration through `vizier run <alias>` file selectors instead of wrapper commands.
- Repo-local stage templates now ship as canonical primitive DAGs (`template.stage.{draft,approve,merge}@v2`) under `.vizier/workflows/*.toml`:
  - draft: `worktree.prepare -> prompt.resolve -> agent.invoke -> plan.persist -> git.stage_commit -> worktree.cleanup -> terminal`, with `plan_branch` + `plan_doc` artifacts produced by `plan.persist`.
  - approve: `worktree.prepare -> prompt.resolve -> agent.invoke -> git.stage_commit -> gate.stop_condition -> worktree.cleanup -> terminal`, with `until_gate` retry back-edge (`stop_gate.on.failed -> stage_commit`).
  - merge: `git.integrate_plan_branch` plus canonical conflict/CI gates (`gate.conflict_resolution`, `gate.cicd`), with stage outcomes settling on gate node statuses (no explicit template-level terminal sink node).
- Templates worktree drift repair (2026-02-15, gate attempt 2): `.vizier/config.toml` re-includes `[commands].draft|approve|merge` file selectors, `.vizier/workflows/{draft,approve,merge}.toml` no longer uses legacy `vizier.*` labels, merge-stage nodes carry explicit `${slug}` args so conflict sentinels stay keyed to `.vizier/tmp/merge-conflicts/<slug>.json`, draft cleanup is wired via explicit `after` from `stage_commit`, and merge `gate.cicd` failed attempts now settle terminally for operator-driven `vizier jobs retry`.
- `vizier run --set` now expands queue-time across the Phase 1 workflow-template surface (not just `nodes.args`): artifact payload fields in `needs`/`produces`, `locks[].key`, custom precondition args, gate script/custom fields, gate bool fields (`approval.required`, `cicd.auto_resolve`), retry mode/budget, and artifact-contract IDs/versions.
- Queue-time interpolation now enforces strict coercion for expanded typed fields: bool tokens (`true|false|1|0|yes|no|on|off`, case-insensitive), retry budget as decimal `u32`, and retry mode via canonical enum parsing; invalid coercions now fail with field-path errors before enqueue.
- Phase 2 topology/identity interpolation (`nodes.after`, `nodes.on.*`, template `id/version`, `imports`, `links`) is explicitly deferred; queue-time expansion currently keeps scheduler graph identity static aside from Phase 1 policy/data fields.
- `vizier run` queue-time orchestration now delegates to `enqueue_workflow_run`, annotates alias metadata (`command_alias`) when alias-invoked, applies root-level `--after` and `--require-approval`/`--no-require-approval` overrides, triggers one scheduler tick, and emits text/json enqueue summaries including `run_id`, selector, template identity, and root job IDs.
- `vizier run --follow` now polls scheduler progression to run-terminal state, streams status/log updates in text mode, and exits deterministically (`0` success, `10` blocked-only terminal set, non-zero when any failed/cancelled job is present).
- Dispatch/actions now include a dedicated `run` orchestration path while removed wrapper action modules remain absent from the CLI build.
- Man-page output now ships only `man1/vizier.1` + `man1/vizier-jobs.1` + `man5/vizier-config.5` + `man7/vizier-workflow.7`; `man1/vizier-build.1` is removed.
- Install/test assets now stage and validate the reduced man-page set.
- Installer now seeds global stage templates (`draft.toml`, `approve.toml`, `merge.toml`) into `WORKFLOWSDIR` (default `<base_config_dir>/vizier/workflows`), preserves pre-existing user templates, and tracks only installed/unchanged-seeded files in uninstall manifests.
- Config hard cut is now active: `[workflow.templates]` and legacy `[agents.<scope>]` sections fail validation; dotted selectors (`template.name.vN`) fail with migration guidance to canonical `template.name@vN`.
- Integration coverage now includes `vizier run` alias/file execution, args and non-args `--set` expansion coverage, queue-time legacy-uses/unresolved-placeholder rejection with no partial enqueue, root `--after`/approval overrides, and follow exit-code semantics, while keeping negative unknown-subcommand coverage for still-removed wrappers.
- Stage integration coverage now also asserts primitive stage behavior: alias-driven draft/approve/merge smoke execution, approve stop-condition retry-loop attempts, merge conflict-gate sentinel preservation, and `vizier jobs` control paths (`tail`, `attach`, `approve`, `cancel`, `retry`) against stage-run job records.
- Kernel workflow-template compilation now emits node classification metadata (`node_class`, `executor_class`, `executor_operation`, `control_policy`) and accepts only canonical `uses` families (`cap.env.*`, `cap.agent.invoke`, `control.*`); compile/validate hard-rejects legacy `vizier.*`, legacy non-env `cap.*`, and unknown implicit labels.
- Canonical executor operation map now treats `cap.agent.invoke` as the single agent runtime primitive with no legacy alias translation path.
- Canonical validator contracts now require:
  - `cap.agent.invoke`: `kind=agent`, exactly one prompt dependency (`custom:prompt_text:<key>`), and no inline `args.command`/`args.script`.
  - `cap.env.builtin.prompt.resolve`: `kind=builtin`, no inline command/script args, exactly one prompt artifact output.
  - `cap.env.shell.prompt.resolve`: `kind=shell|custom`, exactly one of `args.command`/`args.script`, exactly one prompt artifact output.
- Maintained repository template artifacts (`.vizier/workflows/{draft,approve,merge}.toml` plus `.vizier/develop.toml`) now use canonical `uses` IDs only (`cap.env.builtin.plan.persist`, `cap.env.builtin.git.stage_commit`, `cap.env.builtin.git.integrate_plan_branch`) while keeping command-surface behavior unchanged.
- Job metadata paths now treat executor identity fields (`workflow_executor_class`, `workflow_executor_operation`, `workflow_control_policy`) as canonical; `workflow_capability_id` is read-only legacy compatibility for historical records and is no longer propagated as active identity metadata.
- Scheduler/job/workflow runtime side effects now live in `vizier-core/src/jobs/mod.rs`; `vizier-cli/src/jobs.rs` is a thin compatibility re-export so retained `vizier jobs`/`vizier run` behavior and on-disk artifacts remain unchanged.
- Queue-time runtime compilation now persists workflow run manifests under `.vizier/jobs/runs/<run_id>.json` and enqueues one node job per compiled template node with `child_args = ["__workflow-node", "--job-id", "<id>"]`.
- Runtime node execution now records `workflow_run_id`, `workflow_node_attempt`, `workflow_node_outcome`, and `workflow_payload_refs` metadata fields; retry rewind clears outcome/payload refs and increments node attempt.
- Runtime execution root resolution now uses `metadata.execution_root` and otherwise uses repo root; legacy-only `metadata.worktree_path` records now fail with migration guidance. Explicit resolved roots are canonicalized/repo-bounded and fail node execution when missing or outside the repository.
- Workflow runtime metadata now includes `execution_root` alongside worktree ownership fields; `worktree.prepare` writes both ownership metadata and execution-root context, while successful `worktree.cleanup` resets context to `.` and clears worktree ownership fields.
- Runtime node dispatch now implements every canonical executor operation (`prompt.resolve`, `agent.invoke`, `worktree.prepare`, `worktree.cleanup`, `plan.persist`, `git.stage_commit`, `git.integrate_plan_branch`, `git.save_worktree_patch`, `patch.pipeline_prepare`, `patch.execute_pipeline`, `patch.pipeline_finalize`, `build.materialize_step`, `merge.sentinel.write`, `merge.sentinel.clear`, `command.run`, `cicd.run`) and every canonical control policy (`gate.stop_condition`, `gate.conflict_resolution`, `gate.cicd`, `gate.approval`, `terminal`) against repo/worktree-aware execution roots.
- `agent.invoke` no longer uses payload-echo facade behavior; it now resolves configured agent settings/runner, executes the backend, records agent metadata/exit code, and fails node execution on backend failure/timeout.
- Worktree runtime handlers now enforce ownership semantics: `worktree.prepare` creates `.vizier/tmp-worktrees/<purpose>-<job_id>` and records worktree + execution-root metadata; `worktree.cleanup` removes only job-owned safe paths, resets execution-root/worktree metadata on successful cleanup, and records degraded cleanup metadata when prune/remove cannot complete.
- Runtime handlers now materialize canonical operation artifacts/state in-place: `plan.persist` writes plan docs + `.vizier/state/plans/*`, `git.integrate_plan_branch` blocks via merge sentinels on conflicts, patch/build/sentinel operations persist deterministic manifests/markers, and shell/gate operations capture real exit status for outcome routing.
- `on.succeeded` routing now materializes to explicit `after` dependencies at enqueue time (single-parent constraint per target), with route metadata retained for edge-local execution-root propagation; `on.failed` / `on.blocked` / `on.cancelled` routes trigger retry-driven requeue for target nodes.
- Retry rewind now updates execution-root metadata with cleanup outcomes: done/skipped cleanup resets to repo root (`.`), while degraded cleanup preserves worktree + execution-root context for operator recovery.
- `vizier jobs show` metadata surfaces now include `execution_root` for workflow-node records.
- Prompt payload transport now has a typed adjunct store at `.vizier/jobs/artifacts/data/<type_hex>/<key_hex>/<job_id>.json`; custom marker files under `.vizier/jobs/artifacts/custom/...` remain the scheduler gating truth.
- Legacy `draft/*` branches and `.vizier/implementation-plans/*.md` files can still appear in non-bijective states in existing worktrees; treat them as historical residue, not an active command surface.
- Current worktree evidence (`draft/templates`, revalidated 2026-02-15): `.vizier/implementation-plans/` contains `impl.md`, there are no tracked plan-doc deletions in this worktree, and local draft branch inventory is `draft/templates`; drift remains active because the branch/doc state is still non-bijective.

Acceptance checkpoints (selected)
- `vizier --help` / `vizier help --all` list the retained command set (`run` included) and current global flags.
- Each still-removed wrapper command returns generic unknown-subcommand errors without custom migration guidance.
- `vizier run <alias>` and `vizier run file:<path>` both enqueue manifests + node jobs with canonical `workflow_*` metadata and selector persistence.
- `vizier run <flow>` resolves only explicit file/path sources, configured `[commands]` aliases, and canonical selector identities; unresolved flows fail without repo/global fallback discovery.
- `[workflow.global_workflows]` now acts only as the explicit-file allowlist outside repo root; disabling it forbids out-of-repo explicit workflow files.
- `vizier run draft|approve|merge` executes canonical primitive stage DAGs from `.vizier/workflows/*.toml` with no `vizier.*` `uses` labels.
- `install.sh` seeds global stage templates into `WORKFLOWSDIR`, preserves pre-existing user templates, and keeps uninstall parity via manifest tracking.
- `vizier run --set` rewrites compiled node fields across args/artifacts/locks/preconditions/gates/retry/artifact-contract IDs+versions at queue-time, with unresolved placeholders or invalid coercions failing before enqueue and without writing run manifests.
- `vizier run --after` and `vizier run --require-approval` alter root schedule behavior without reviving any removed global flags.
- `vizier run --follow` returns deterministic terminal exits (`0`, `10`, non-zero) for success/blocked/failure aggregates.
- `vizier jobs approve|retry|cancel|tail|attach` remains operable against stage-run jobs created by `vizier run draft|approve|merge`.
- `vizier help --pager` and `vizier list --pager` fail as unknown-argument paths (with Clap tip toward hidden `--no-pager`), preventing accidental resurrection of a user-facing pager global.
- Generated man pages and installation manifests contain no `vizier-build` page and no removed command inventory.
- Workflow-template validator coverage asserts executor/control classification, canonical prompt->invoke validation, hard rejection of legacy `vizier.*` and legacy non-env `cap.*` labels, and rejection of unknown implicit `uses` labels.
- `vizier jobs show` surfaces canonical executor identity metadata (`agent.invoke`) for new records while remaining tolerant of historical job records that still carry legacy capability fields on disk.
- Workflow runtime coverage asserts queue-time node-job materialization, prompt payload roundtrip (`prompt.resolve` producer to `agent.invoke` consumer), per-operation canonical runtime execution (success/failure and artifact contracts across all canonical executor operations/control policies), stop-condition retry-budget blocking behavior, and payload cleanup during retry rewind.
- Workflow runtime coverage now also asserts execution-root precedence/safety, route-time context propagation idempotence + running-target guards, retry-time propagated context injection, and cleanup reset propagation across `vizier run` success edges.
- `vizier help --all` continues to hide `__workflow-node` while `vizier __workflow-node --job-id <id>` remains executable for scheduler children.
- `cargo check --all --all-targets`, `cargo test --all --all-targets`, and `./cicd.sh` pass on this branch.

Next moves
1) Decide whether runtime `on.succeeded` single-parent constraint should be generalized to multi-parent routing without weakening scheduler determinism.
2) Decide where/when scheduler enqueue paths should auto-populate runtime run IDs/selectors for external enqueue callers that bypass `vizier run` and template compilation helpers.
3) Decide whether/when to ship deferred Phase 2 `--set` topology/identity interpolation (`after`/`on`, template identity, imports, links) under deterministic graph constraints.
4) Keep pruning now-unreachable wrapper-era internals while preserving compatibility for persisted job artifacts.
5) Define archive cleanup policy for legacy `draft/*` branch and `.vizier/implementation-plans/*.md` drift so retained plan-visibility surfaces stay interpretable after workflow removal.
6) Reconcile operator guidance with actual help-pager flags (`--pager` currently documented in AGENTS but rejected by CLI; hidden `--no-pager` remains internal).
