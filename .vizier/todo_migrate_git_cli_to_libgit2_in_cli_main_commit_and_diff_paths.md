Resolve tight coupling to external git CLI by replacing command invocations with libgit2 (git2 crate) across commit and diff flows in cli/src/main.rs.

Current tension
- The CLI shells out to `git` for core operations (staging, commit, diff). This breaks portability, complicates error handling, and prevents structured diagnostics.
- Code references:
  - Diff generation for save range: cli/src/main.rs: around lines 146–169 (`git diff <range> -- :!.vizier/` and HEAD path)
  - Staging and commit: cli/src/main.rs: ~77–94 (`git add -u`, `git commit -m <msg>`)

Acceptance criteria
- Introduce git2 dependency in cli/Cargo.toml and use it to:
  1) Produce a unified diff string excluding `.vizier/` for both `-s <REF|RANGE>` and `-S` paths.
  2) Stage currently tracked changes equivalent to `git add -u`.
  3) Create a commit with LLM-generated message, preserving author/committer from environment (GIT_AUTHOR_NAME/EMAIL, etc.) or git config fallback.
  4) Robust error propagation with contextual messages (no silent failures). Return same diff text semantics as current path so commit prompt remains stable.

Implementation notes
- Diff
  - Open repo at project root with git2::Repository::discover.
  - For `-s <RANGE>`: parse `A..B` into two OIDs via revparse. For single `<REF>` compare `<REF>`..WORKDIR.
  - Build DiffOptions with pathspec `:!.vizier/` equivalent: set pathspec to ["."] and add .vizier to a pathspec exclude via `set_pathspec` + `set_ignore_submodules(true)` and use a custom `diff::Delta` filter to skip entries where path starts_with ".vizier/".
  - Generate patch text via `repo.diff_tree_to_workdir_with_index` and/or `diff_tree_to_tree` depending on mode. Serialize with `print`/`format_email`/iterate hunks and lines to collect into a unified string.
- Stage tracked changes (git add -u)
  - Obtain index via repo.index(). Update for working tree with `add_all(["*"], IndexAddOption::UPDATE, Some(filter_cb))` where filter_cb skips `.vizier/*`.
  - Write index.
- Commit
  - If no changes in index vs HEAD, short-circuit with message.
  - Create tree from index; parents: HEAD commit if exists.
  - Signature from `Signature::now` using config values or env; fallback to repo default.
  - `repo.commit(Some("HEAD"), &author, &committer, &message, &tree, &parents)`.

Telemetry and UX
- Replace ad-hoc prints with existing auditor logging hooks; preserve "Changes committed with message: ..." log.

Migration steps
- Add git2 = "0.18" to cli/Cargo.toml.
- Implement helper module cli/src/vcs.rs with functions:
  - get_diff_string(range: Option<String>) -> Result<String>
  - stage_tracked_excluding_vizier() -> Result<()>
  - commit_index(message: &str) -> Result<()> 
- Refactor cli/src/main.rs save paths to call these helpers.

Edge cases to cover in tests/manual
- Repository with no HEAD (initial commit) — allow committing staged files.
- Empty diff -> skip LLM commit path gracefully.
- Worktrees and submodules — ensure we ignore submodules in diff.
- Non-UTF8 filenames — ensure diff generation doesn’t panic; fallback to lossy display in prompt.

Definition of done
- No remaining uses of std::process::Command("git") in cli crate.
- `vizier -s HEAD~3..HEAD` and `vizier -S` produce identical diffs pre/post change for typical ASCII paths.
- Commit works on macOS/Linux in repos without global git config set, using env fallbacks.