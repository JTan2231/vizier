# Gates And Conflicts

This page covers operational checks for the retained command surface.

## Initialization Gate

Use `vizier init --check` in CI to fail fast when the repo is missing required Vizier marker files or `.gitignore` runtime entries.

## Job Safety Controls

`vizier jobs` supports explicit approval/rejection and retry/cancel controls for queued/running records.

## Release Safety

`vizier release` enforces repository preconditions (clean worktree, branch state, no in-progress merge/rebase/cherry-pick) before writing release artifacts.

## Deprecated Workspace Commands

`vizier cd` and `vizier clean` intentionally fail with deprecation errors; scheduler-managed temp worktrees remain the source of truth.
