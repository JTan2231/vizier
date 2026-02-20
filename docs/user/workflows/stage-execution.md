# Stage Execution

Stage orchestration runs through repo-local workflow templates plus scheduler primitives:

- `vizier run draft`
- `vizier run approve`
- `vizier run merge`
- `vizier jobs ...` (status/tail/attach/approve/retry/cancel/gc)

No top-level `vizier draft|approve|merge` wrappers are part of the active CLI surface.

## Stage Template Contracts

Stage templates live at:

- `.vizier/workflows/draft.hcl`
- `.vizier/workflows/approve.hcl`
- `.vizier/workflows/merge.hcl`

The repository-shipped stage templates are `template.stage.*@v2`.

Each template must use canonical `uses` IDs only:

- executor nodes: `cap.env.*`, `cap.agent.invoke`
- control nodes: `control.*`

Legacy `vizier.*` labels fail queue-time validation before any jobs or run manifests are created.

## Alias Mapping

Stage aliases should be mapped in `.vizier/config.toml` so `vizier run <alias>` resolves to repo-local stage files:

```toml
[commands]
draft = "file:.vizier/workflows/draft.hcl"
approve = "file:.vizier/workflows/approve.hcl"
merge = "file:.vizier/workflows/merge.hcl"
```

## Canonical Stage Shapes

- `draft`: `worktree_prepare -> resolve_prompt -> invoke_agent -> persist_plan -> stage_files -> stage_commit -> stop_gate -> worktree_cleanup -> terminal`
- `approve`: `worktree_prepare -> resolve_prompt -> invoke_agent -> stage_files -> stage_commit -> stop_gate -> worktree_cleanup -> terminal`
- `merge`: `merge_integrate -> merge_gate_cicd -> terminal`, with `merge_integrate.on.blocked -> merge_conflict_resolution`

## Cross-Run Dependency Contracts

Shipped stage templates now opt into optimistic artifact dependency waiting:

```toml
[policy.dependencies]
missing_producer = "wait"
```

Stage contract details:

- `draft.persist_plan` produces `plan_branch:{slug,branch}` and `plan_doc:{slug,branch}`.
- `approve.worktree_prepare` needs `plan_branch:{slug,branch}` and `plan_doc:{slug,branch}`.
- `approve.terminal` produces `custom:stage_token:approve:${slug}`.
- shipped `merge.merge_integrate` needs `custom:stage_token:approve:${slug}`.
- custom merge variants can also add `plan_branch:{slug,branch}` when explicit source-branch presence gating is required.
- `approve`/`merge` declare the `stage_token@v1` artifact contract so custom stage tokens validate at queue time.

With these contracts in place, `vizier run draft`, `vizier run approve`, and `vizier run merge` can be queued back-to-back without explicit `--after` wiring.

## Operational Notes

- `vizier run` accepts template params via `--set key=value`, named flags (`--spec-file`, `--slug`, ...), or ordered positional inputs declared by template `[cli].positional`.
- Named flags map kebab-case to snake_case (`--spec-file` -> `spec_file`); templates may also define `[cli].named` aliases for friendlier entry labels (for example, stage draft supports `--name` -> `slug` and `--file` -> `spec_file`).
- Stage draft snapshots `spec_file` contents into `persist_plan.args.spec_text` at enqueue time when `spec_source=inline` and `spec_text` is empty, so uncommitted local specs can be used safely.
- Stage `plan.persist` now explicitly stages the generated `.vizier/implementation-plans/<slug>.md` path via VCS helpers, so draft plans remain commit-visible even when `.vizier/implementation-plans` is ignored.
- Stage prompt files are hardcoded in the shipped templates:
  - draft: `.vizier/prompts/DRAFT_PROMPTS.md`
  - approve: `.vizier/prompts/APPROVE_PROMPTS.md`
  - merge companion: `.vizier/prompts/MERGE_PROMPTS.md`
- `prompt.resolve` now renders `{{placeholder}}` tokens found in prompt text and requires every placeholder to resolve.
- Placeholder resolution sources are generic: current node args (`{{key}}`), any run-manifest node arg (`{{node_id.arg_key}}`), and file includes (`{{file:relative/or/absolute/path}}`).
- In composed workflows (for example `develop` imports), `prompt.resolve` also resolves same-stage local aliases (`{{persist_plan.spec_text}}`) in addition to fully-qualified namespaced keys (`{{develop_draft__persist_plan.spec_text}}`).
- Unresolved prompt placeholders fail the `resolve_prompt` node with an explicit error.
- Entry-node preflight now reports missing root inputs before enqueue, including actionable examples derived from `[cli].positional`/`[cli].named`.
- Stage `worktree_prepare` defaults to `draft/<slug>` when `branch` is unset; provide `branch` explicitly to override.
- Stage `merge_integrate` also defaults to `draft/<slug>` when source branch args are unset; `vizier run merge <slug>` can run without explicitly setting `branch`.
- Stage `merge_conflict_resolution` now defaults `conflict_auto_resolve=true`; when merge conflicts are detected it attempts configured agent-based resolution before remaining blocked for operator retry.
- During `merge_integrate`, Vizier now loads `.vizier/implementation-plans/<slug>.md` from the source branch (or source history fallback), appends that content to the merge commit message body, and commits removal of the plan doc from the source branch tip before finalizing merge integration.
- Queue-time capability validation now enforces executor arg contracts before any jobs are enqueued. Examples: `worktree.prepare` requires one of `args.branch|args.slug|args.plan`; `git.integrate_plan_branch` requires one of `args.branch|args.source_branch|args.plan_branch|args.slug|args.plan`; `cicd.run` requires `args.command`/`args.script` or a non-empty `cicd` gate script; `patch.pipeline_prepare` and `patch.execute_pipeline` require `args.files_json`.
- `vizier run --set` still applies queue-time interpolation and typed coercion before enqueue.
- `vizier run --repeat <N>` applies to stage aliases as well (`draft`, `approve`, `merge`), enqueuing repeated stage runs in strict sequence by chaining each iteration on the previous iteration's success sinks.
- `vizier run --after`, `--require-approval`, and `--follow` remain available stage orchestration controls.
- Job log streaming is command-local: `vizier jobs tail <job> --follow`.
- Help output auto-pages only on TTY (using `$VIZIER_PAGER` when set, otherwise the fallback pager) and prints directly on non-TTY output.
- Explicit `--pager` is unsupported; hidden `--no-pager` is internal-only.
