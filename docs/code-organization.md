# Code organization overview

This refactor splits previously oversized Rust sources into focused modules. Behavior and public APIs remain unchanged; only file layout moved.

## CLI actions
- `vizier-cli/src/actions/` holds per-command handlers (`ask`, `save`, `draft`, `refine`, `approve`, `review`, `merge`, `list`, `plan`, `snapshot_init`, `test_display`).
- Shared helpers live in `vizier-cli/src/actions/shared.rs` and option/type definitions in `vizier-cli/src/actions/types.rs`.
- Cross-command context + errors are in `vizier-cli/src/context.rs` and `vizier-cli/src/errors.rs`.

## Core config
- `vizier-core/src/config/` is split by responsibility:
  - `schema.rs` (types + core logic)
  - `prompts.rs` (prompt enums + selection metadata)
  - `defaults.rs` (default implementations)
  - `merge.rs` (layer merge logic)
  - `load.rs` (config parsing + path helpers)
  - `validate.rs` (config tests)

## Core VCS helpers
- `vizier-core/src/vcs/` is split by domain:
  - `branches.rs`, `worktrees.rs`
  - `status.rs` (diff/status helpers)
  - `commits.rs` (stage/commit helpers)
  - `merge.rs` (merge + conflict helpers)
  - `remotes.rs` (push/auth + remote parsing)
  - `tests.rs` (vcs unit tests)

## Integration tests
- `tests/src/fixtures.rs` hosts shared fixtures/utilities.
- Per-workflow tests live in dedicated modules (`ask.rs`, `save.rs`, `draft.rs`, `approve.rs`, `review.rs`, `merge.rs`, `workspace.rs`, etc.).
- `tests/src/lib.rs` is a thin module list that re-exports fixtures.
