# Stage Execution

Stage orchestration runs through repo-local workflow templates plus scheduler primitives:

- `vizier run draft`
- `vizier run approve`
- `vizier run merge`
- `vizier jobs ...` (status/tail/attach/approve/retry/cancel/gc)

No top-level `vizier draft|approve|merge` wrappers are part of the active CLI surface.

## Stage Template Contracts

Stage templates live at:

- `.vizier/workflow/draft.toml`
- `.vizier/workflow/approve.toml`
- `.vizier/workflow/merge.toml`

Each template must use canonical `uses` IDs only:

- executor nodes: `cap.env.*`, `cap.agent.invoke`
- control nodes: `control.*`

Legacy `vizier.*` labels fail queue-time validation before any jobs or run manifests are created.

## Alias Mapping

Stage aliases should be mapped in `.vizier/config.toml` so `vizier run <alias>` resolves to repo-local stage files:

```toml
[commands]
draft = "file:.vizier/workflow/draft.toml"
approve = "file:.vizier/workflow/approve.toml"
merge = "file:.vizier/workflow/merge.toml"
```

## Canonical Stage Shapes

- `draft`: `worktree.prepare -> prompt.resolve -> agent.invoke -> plan.persist -> git.stage_commit -> worktree.cleanup -> terminal`
- `approve`: `worktree.prepare -> prompt.resolve -> agent.invoke -> git.stage_commit -> gate.stop_condition -> worktree.cleanup -> terminal`
- `merge`: `git.integrate_plan_branch` plus optional `gate.conflict_resolution` and `gate.cicd` routing before `terminal`

## Operational Notes

- `vizier run --set` applies queue-time interpolation and typed coercion before enqueue.
- `vizier run --after`, `--require-approval`, and `--follow` are the stage orchestration controls.
- Job log streaming is command-local: `vizier jobs tail <job> --follow`.
- Help output is pager-aware on TTY and plain in non-TTY contexts.
