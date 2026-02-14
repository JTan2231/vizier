# Gates and Conflicts

This page covers policy gates, merge-conflict recovery, and operational failure handling for the plan workflow.

For stage mechanics (`draft`, `approve`, `review`, `merge`), see `docs/user/workflows/stage-execution.md`.

## Gates and policy checks

Workflow outcomes can be gated by repository policy:

- `review` checks from `[review.checks]`.
- Merge CI/CD gate from `[merge.cicd_gate]`.
- Approve loop stop-condition from `[approve.stop_condition]`.

### Review checks

`vizier review` runs configured checks before critique and includes failures in the critique context.
If `[review.checks]` is unset in Cargo repos, it defaults to:

- `cargo check --all --all-targets`
- `cargo test --all --all-targets`

Use `--skip-checks` to bypass local checks for one run.

### Merge CI/CD gate

When `[merge.cicd_gate].script` is set, merge runs that script from repo root:

- Squash mode: gate runs while implementation commit is staged, before merge commit is written.
- No-squash mode: gate runs after merge commit creation.

A non-zero exit blocks completion and preserves artifacts for debugging.
Optional auto-remediation is controlled by `[merge.cicd_gate].auto_resolve` and `retries`, with per-run overrides:

- `--cicd-script PATH`
- `--cicd-retries N`
- `--auto-cicd-fix` / `--no-auto-cicd-fix`

`vizier review` also executes the gate once for visibility, but with auto-remediation disabled.

### Approve stop-condition

`[approve.stop_condition]` (or `--stop-condition-script`/`--stop-condition-retries`) runs a repo-local script after each approve attempt.

- Exit 0: approve loop ends.
- Non-zero: approve retries agent application until retry budget is exhausted.

With `retries = 3`, approve can run the agent up to four times total.

## Merge conflicts and resume

When merge integration hits conflicts, Vizier writes a sentinel under:

- `.vizier/tmp/merge-conflicts/<slug>.json`

Resume paths:

- Manual: resolve conflicts, stage files, then run `vizier merge <slug> --complete-conflict`.
- Auto: run with `--auto-resolve-conflicts` (or enable `[merge.conflicts].auto_resolve`) so the selected agent attempts conflict cleanup.

`--complete-conflict` fails fast when no pending sentinel exists, when Git is not in merge state, or when you are on the wrong target branch.

## Failure and recovery playbook

| Situation | Recovery |
| --- | --- |
| Failed/blocked scheduler segment | Fix root cause and run `vizier jobs retry <job-id>`. Vizier rewinds that job and downstream dependents. |
| Draft worktree creation fails | Fix error and rerun `vizier draft`; stub branches are cleaned unless plan commit already exists. |
| `vizier approve` fails mid-run | Inspect preserved temp worktree, salvage changes, rerun when corrected. |
| Merge conflicts | Resolve on target branch, stage files, rerun `vizier merge <slug> --complete-conflict`. |
| Need to resume after aborting Git merge | If sentinel remains and worktree is clean, rerun `vizier merge <slug>`. |
| CI/CD gate failure | Merge stays blocked; inspect script output, fix manually or retry with auto-fix enabled, rerun merge when gate passes. |
| Backend auto-resolution fails | Follow manual conflict workflow; resolve, stage, and retry. |

## End-to-end walkthrough

1. Draft:
   ```bash
   vizier draft --file specs/ingest.md
   ```
2. Approve:
   ```bash
   vizier approve ingest-backpressure
   git diff main...draft/ingest-backpressure
   ```
3. Review:
   ```bash
   vizier review ingest-backpressure
   ```
4. Merge:
   ```bash
   vizier merge ingest-backpressure
   ```

Need to keep the branch after merge? add `--keep-branch`.
Hit conflicts? resolve and rerun with `--complete-conflict`.

## Shipping a local release

After plan work lands on target, create a local release commit/tag from history:

```bash
vizier release --dry-run
vizier release --yes
```

`vizier release` is local-history-only in MVP (no hosted forge publishing). See `docs/user/release.md`.

## FAQ

Can I run `vizier approve` without re-drafting?
Yes. If you edit `.vizier/implementation-plans/<slug>.md` on the draft branch, rerun `vizier approve <slug>`.

What if the draft branch lags far behind target?
`vizier approve` warns. Rebase/merge the draft branch first, or resolve conflicts later during merge.

Does `vizier merge` push automatically?
No. Use global `--push` when you want successful runs pushed, or push manually.

## Architecture docs and multi-agent workflows

Plan documents under `.vizier/implementation-plans/` do not replace architecture-doc references required by compliance gates.
For multi-agent collaboration, keep one plan slug and consistent architecture-doc linkage across draft/approve/review/merge outputs.
