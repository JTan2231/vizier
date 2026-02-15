# Libgit runtime no-git-cli

Status (2026-02-15): ACTIVE. Runtime and test Git operations are now libgit2-only; keep parity and cleanup semantics stable.

Thread
- Remove all direct `git` subprocess spawning from workflow runtime and test harness paths while preserving stage/merge/patch/worktree behavior and retry cleanup metadata contracts.

What changed
- `vizier-core/src/jobs/mod.rs` runtime handlers now use `vizier-core/src/vcs/*` path-scoped helpers for branch/worktree/merge/blob/patch operations instead of `git -C ...` subprocess calls.
- Retry worktree cleanup no longer uses `git worktree remove/prune` fallback commands; cleanup now retries libgit2 prune using provided/derived worktree-name candidates and reports degraded cleanup when no registered worktree matches.
- `vizier-core/src/vcs/*` gained path-scoped helper coverage for branch state, merge preparation/commit, blob lookup/read, path revision walking, binary diff generation, and index-aware patch apply.
- Integration/runtime tests and fixtures removed `Command::new("git")` usage and now run fixture Git operations through libgit2-backed helpers.
- Shared-branch worktree creation now has libgit2 force-equivalent behavior: when the branch is already checked out in another linked worktree, runtime adds via a temporary branch and repoints the new worktree HEAD to the requested branch.
- Branch checkout helpers now include linked-worktree fallback (detach then reattach) so merge plan-doc cleanup can switch onto a source branch even when that branch is active in another linked worktree.
- Binary patch output now reconstructs diff hunk origins (`+/-/ `) before writing patch bytes, restoring compatibility with libgit2 `Diff::from_buffer` apply flows used by patch pipeline runtime/tests.
- `branch_exists()` now discovers the repository from CWD (instead of requiring `.` to be the repo root), restoring subdirectory-safe slug-uniqueness checks in CLI plan helpers.

Parity risks to watch
- Binary diff/apply output parity versus `git diff --binary` and `git apply --index` in edge cases (renames, mode changes, binary blobs).
- Merge no-op behavior (`already up to date`) and conflict materialization details when switching from CLI merges to libgit2 merge-preparation/commit paths.
- Worktree cleanup degradation wording/semantics on unusual repository layouts.
