# Code organization overview

This refactor splits previously oversized Rust sources into focused modules. Behavior and public APIs remain unchanged; only file layout moved.

## CLI actions
- `vizier-cli/src/actions/` holds per-command handlers (`ask`, `save`, `draft`, `approve`, `review`, `merge`, `list`, `plan`, `snapshot_init`, `test_display`).
- Shared helpers live in `vizier-cli/src/actions/shared.rs` and option/type definitions in `vizier-cli/src/actions/types.rs`.
- Cross-command context + errors are in `vizier-cli/src/context.rs` and `vizier-cli/src/errors.rs`.
- `vizier-cli/src/cli/` contains CLI-only wiring: argument parsing, help/pager rendering, prompt input resolution, job list/show formatting, and scheduler/background orchestration helpers.

## Kernel vs drivers
- `vizier-kernel/` is the pure domain crate: scheduler semantics, config schema/defaults/merge, prompt templates + assembly, audit/outcome data types, and port traits.
- `vizier-core/` remains the driver host: config resolution/precedence, prompt-context loading, agent execution, VCS, display, and filesystem orchestration.

## Config
- `vizier-kernel/src/config/` holds schema + defaults + merge logic shared by all frontends.
- `vizier-core/src/config/` handles driver-specific resolution:
  - `driver.rs` (agent runtime resolution, runner wiring, prompt-profile resolution)
  - `load.rs` (config parsing + path helpers + repo prompt discovery)
  - `validate.rs` (config parsing/resolution tests)

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
- Scheduler/job coverage lives in `tests/src/background.rs` (scheduler flows, failure paths) and `tests/src/jobs.rs` (list/show/status/tail/attach/gc formatting and cleanup).
- `tests/src/lib.rs` is a thin module list that re-exports fixtures.
- Scheduler spec tests live under `vizier-kernel/src/scheduler/spec.rs` (pure kernel coverage).
- The global integration-test lock in `tests/src/fixtures.rs` should guard tests that spawn external processes (including install/shim tests) to avoid parallelism flakes.

## Conditional compilation
- The agent runner has unix-only execution paths plus a runtime mock path used during integration testing (`VIZIER_MOCK_AGENT=1` when the `integration_testing` feature is enabled).
