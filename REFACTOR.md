# Refactor Spec: Code Organization Decomposition

Status: Proposed
Owner: TBD
Target: No behavior changes; structure-only refactor

## Summary
The repository is functionally organized at the crate level (CLI vs core vs tests), but several files have grown large enough to become navigation and review bottlenecks (notably `vizier-cli/src/actions.rs`, `vizier-cli/src/main.rs`, `vizier-core/src/config.rs`, `vizier-core/src/vcs.rs`, and `tests/src/lib.rs`).

This refactor splits those files into focused modules while preserving the existing public APIs and behavior. The intent is to reduce cognitive load, improve ownership boundaries, and make future changes smaller and easier to review.

## Goals
- Reduce file size and improve discoverability of command workflows and core subsystems.
- Keep behavior identical: no CLI surface or runtime changes.
- Preserve public APIs within each crate (re-export where needed).
- Make unit/integration tests easier to locate by workflow.

## Non-Goals
- No new features or behavior changes.
- No CLI flag renames or breaking changes.
- No logic rewrites or algorithm changes.
- No changes to `.vizier/narrative/*`, `AGENTS.md`, or `README.md`.

## Proposed Structure

### 1) CLI: split actions by command
Current: `vizier-cli/src/actions.rs` (~6k LOC) is a single command handler file.

Proposed layout:
```
vizier-cli/src/actions/
  mod.rs
  ask.rs
  save.rs
  draft.rs
  refine.rs
  approve.rs
  review.rs
  merge.rs
  list.rs
  help.rs
  status.rs
```

`vizier-cli/src/actions/mod.rs` exports the command entry points via `pub(crate) use ...`.

Expected benefits:
- Each command lives in its own file.
- Smaller diffs and reduced merge conflicts.
- Clearer mapping between CLI subcommands and implementation.

### 2) CLI: extract shared context and errors
Add shared context and error modules:
```
vizier-cli/src/context.rs
vizier-cli/src/errors.rs
```

Example `CliContext` fields:
- `repo_root`
- `config`
- `display` settings
- `agent` settings

This reduces duplicated setup code across command handlers.

### 3) Core: split config by responsibility
Current: `vizier-core/src/config.rs` (~2.9k LOC).

Proposed layout:
```
vizier-core/src/config/
  mod.rs
  schema.rs
  load.rs
  merge.rs
  defaults.rs
  validate.rs
  prompts.rs
```

`mod.rs` re-exports the existing `config` API so call sites do not change.

### 4) Core: split VCS helpers by domain
Current: `vizier-core/src/vcs.rs` (~3.2k LOC).

Proposed layout:
```
vizier-core/src/vcs/
  mod.rs
  branches.rs
  worktrees.rs
  merge.rs
  status.rs
  commits.rs
  remotes.rs
```

`mod.rs` re-exports the existing `vcs` API so call sites do not change.

### 5) Tests: split integration tests by workflow
Current: `tests/src/lib.rs` (~5.2k LOC) contains all integration tests.

Proposed layout:
```
tests/src/
  lib.rs
  fixtures.rs
  ask.rs
  save.rs
  draft.rs
  approve.rs
  review.rs
  merge.rs
  workspace.rs
```

`lib.rs` exposes shared fixtures and `mod` declarations. Each workflow’s tests live in its own module.

## Migration Plan (Incremental)

1) CLI actions split
- Create `vizier-cli/src/actions/`.
- Move each command handler into its own file.
- Keep signatures stable; re-export from `actions/mod.rs`.
- Update `main.rs` call sites only if needed.

2) CLI shared context/error modules
- Extract shared setup into `context.rs`.
- Replace duplicated setup in command modules.

3) Core config split
- Introduce `vizier-core/src/config/`.
- Move code into submodules by responsibility.
- `config/mod.rs` re-exports the same types/functions.

4) Core VCS split
- Introduce `vizier-core/src/vcs/`.
- Move helpers into the relevant submodule.
- `vcs/mod.rs` re-exports existing API.

5) Tests split
- Create `tests/src/fixtures.rs` for shared setup.
- Move tests into per-workflow modules.
- Update `lib.rs` to include modules.

## Acceptance Criteria
- All existing tests pass with no changes to behavior.
- Command help output and CLI behavior are unchanged.
- Public API surface for `vizier-core` remains stable (where used by `vizier-cli`).
- File sizes of the largest offenders drop substantially (target: <1500 LOC per file).

## Risks and Mitigations
- Risk: circular module dependencies after splitting.
  - Mitigation: keep `mod.rs` as thin export layers; use `pub(crate)` where possible.
- Risk: accidental public API change.
  - Mitigation: re-export existing items from `mod.rs` and avoid renaming.
- Risk: large mechanical diff.
  - Mitigation: split in phases; keep diffs small and per-area.

## Testing & Verification
- When code changes occur, run `./cicd.sh`.
- Additionally run:
  - `cargo check --all --all-targets`
  - `cargo test --all --all-targets`

## Out of Scope
- No runtime changes.
- No formatting overhauls.
- No changes to documentation other than this spec.
