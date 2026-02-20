# Gates And Conflicts

This page covers operational checks for the retained command surface.

## Initialization Gate

Use `vizier init --check` in CI to fail fast when the repo is missing required Vizier scaffold files or `.gitignore` runtime entries.
The init contract covers `.vizier/narrative/{snapshot,glossary}.md`, `.vizier/config.toml`, `.vizier/workflows/{draft,approve,merge,commit}.hcl`, root `ci.sh`, and required `.vizier/*` ignore rules.

## Job Safety Controls

`vizier jobs` supports explicit approval/rejection and retry/cancel controls for queued/running records.

## Release Safety

`vizier release` enforces repository preconditions (clean worktree, branch state, no in-progress merge/rebase/cherry-pick) before writing release artifacts.

## Cleanup Safety Gates

`vizier clean <job-id>` enforces scheduler-safety checks before deleting runtime data.

- Default safety refusal (exit `10`) occurs when:
  - any scoped job is active (`queued`, `waiting_on_*`, `running`),
  - a non-scoped active job has an `after` dependency on a scoped job,
  - a non-scoped active job depends on artifacts produced only by scoped jobs.
- `--force` bypasses dependency/reference guards but still refuses unsafe filesystem paths.
- Worktree cleanup only touches job-owned paths under `.vizier/tmp-worktrees/`.
- Branch cleanup only targets eligible local `draft/*` branches and never removes currently checked-out/protected branches.
