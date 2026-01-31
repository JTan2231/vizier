---
plan: refactor
branch: draft/refactor
---

## Operator Spec
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

## Implementation Plan
## Overview
This is a structure-only refactor to decompose several oversized files into focused modules while preserving existing behavior and public APIs. The primary impact is on maintainers and reviewers: navigation, ownership, and review diffs become smaller and more focused without changing any CLI surfaces or runtime behavior. The work is needed now because `vizier-cli/src/actions.rs`, `vizier-cli/src/main.rs`, `vizier-core/src/config.rs`, `vizier-core/src/vcs.rs`, and `tests/src/lib.rs` are large enough to slow review and increase merge conflicts.

## Execution Plan
1. **CLI actions split (primary surface: `vizier-cli/src/actions.rs`)**
   - Create `vizier-cli/src/actions/` with `mod.rs` re-exporting existing command entry points so `vizier-cli/src/main.rs` can keep using the same imports.
   - Move each command’s handler and command-specific helpers into its own module (ask/save/draft/refine/approve/review/merge/list/help). Add modules for current command handlers not listed in the proposal (plan, init-snapshot, test-display, cd/clean) or explicitly document where they live to avoid orphaned logic.
   - Keep cross-command helpers (formatting, gate runners, common option parsing) in a small shared section (`actions/mod.rs` or a minimal shared submodule) to avoid duplication and circular dependencies.

2. **CLI shared context + errors**
   - Introduce `vizier-cli/src/context.rs` with a lightweight `CliContext` (repo root, resolved config, display/verbosity, agent overrides) to remove duplicated setup across command modules.
   - Move shared error types such as `CancelledError` (currently in `vizier-cli/src/actions.rs`) into `vizier-cli/src/errors.rs`, keeping error behavior unchanged.

3. **Core config split (surface: `vizier-core/src/config.rs`)**
   - Create `vizier-core/src/config/` with submodules `schema.rs`, `load.rs`, `merge.rs`, `defaults.rs`, `validate.rs`, `prompts.rs`.
   - Move existing code into those modules and keep `vizier-core/src/config/mod.rs` re-exporting the current public API so downstream uses of `vizier_core::config` remain unchanged.
   - Relocate the `#[cfg(test)]` block from `config.rs` into an equivalent tests module under the new structure without changing test logic.

4. **Core VCS split (surface: `vizier-core/src/vcs.rs`)**
   - Create `vizier-core/src/vcs/` with submodules `branches.rs`, `worktrees.rs`, `merge.rs`, `status.rs`, `commits.rs`, `remotes.rs`.
   - Move VCS types and helpers into the appropriate modules and re-export from `vizier-core/src/vcs/mod.rs` to preserve the existing `vizier_core::vcs` API surface.
   - Move the `#[cfg(test)]` tests from `vcs.rs` into a module under the new layout, keeping the same test coverage and helpers.

5. **Integration test split (surface: `tests/src/lib.rs`)**
   - Extract shared fixtures (e.g., `IntegrationRepo`, helper functions, lock) into `tests/src/fixtures.rs` and re-export them from `tests/src/lib.rs`.
   - Move tests into per-workflow modules (`ask.rs`, `save.rs`, `draft.rs`, `refine.rs`, `approve.rs`, `review.rs`, `merge.rs`, `workspace.rs`). Add additional modules for existing non-workflow tests (config/plan/help/jobs/background/test-display/install) so no tests remain stranded in `lib.rs`.
   - Ensure `tests/src/lib.rs` becomes a thin module list + `pub use fixtures::*;` without behavior changes.

6. **Acceptance alignment and cleanup**
   - Confirm the file-size target (<1500 LOC) for the previously oversized files as an observable acceptance signal.
   - Ensure `vizier-cli/src/main.rs` and external crates still compile against unchanged public APIs.
   - Keep refactor changes mechanical (moves + re-exports), avoiding logic rewrites.

## Risks & Unknowns
- **Module cycles or visibility breaks**: Splitting large files risks circular dependencies or missing re-exports. Mitigation: keep `mod.rs` files as thin export layers and centralize shared helpers to avoid cross-module cycles.
- **Accidental API drift**: Moving items could inadvertently change public visibility or paths. Mitigation: keep `pub`/`pub(crate)` usage consistent and validate downstream compile sites (especially `vizier-cli` and tests).
- **Test relocation churn**: Integration tests are extensive and cross-cutting; careless moves can break helper visibility. Mitigation: create a single `fixtures.rs` and re-export from `lib.rs`.
- **Docs update requirement vs spec**: Repo guidance says code changes require docs updates, while the operator spec forbids doc changes. This needs a decision: either agree that structural refactors are exempt, or identify a minimal non-README/non-AGENTS doc update that is acceptable.

## Testing & Verification
- `./cicd.sh`
- `cargo check --all --all-targets`
- `cargo test --all --all-targets`
- Confirm CLI help output stays consistent via existing integration tests (help/plan/ask/save/review/merge tests) after module moves.

## Notes
- Narrative files remain unchanged per the operator spec and current instructions.
