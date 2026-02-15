# Code organization overview

This refactor splits previously oversized Rust sources into focused modules. Behavior and public APIs remain unchanged; only file layout moved.

## CLI actions
- `vizier-cli/src/actions/` holds per-command handlers (`build`, `save`, `draft`, `approve`, `review`, `merge`, `release`, `list`, `plan`, `test_display`).
- Shared helpers live in `vizier-cli/src/actions/shared.rs` and option/type definitions in `vizier-cli/src/actions/types.rs`.
- Cross-command context + errors are in `vizier-cli/src/context.rs` and `vizier-cli/src/errors.rs`.
- `vizier-cli/src/cli/` contains CLI-only wiring: argument parsing, help/pager rendering, prompt input resolution, command dispatch, and jobs list/show/watch formatting.

## Kernel vs drivers
- `vizier-kernel/` is the pure domain crate: scheduler semantics, config schema/defaults/merge, prompt templates + assembly, audit/outcome data types, and port traits.
- `vizier-core/` remains the driver host: config resolution/precedence, prompt-context loading, agent execution, VCS, display, filesystem orchestration, and scheduler/job/workflow runtime side effects.

## Jobs ownership
- `vizier-core/src/jobs/mod.rs` owns scheduler/job lifecycle orchestration, persistence, workflow enqueue/runtime dispatch, retry/approval/cancel/gc operations, and log helpers.
- `vizier-core/src/plan.rs` provides reusable plan-domain helpers consumed by workflow runtime handlers (`plan.persist`, merge plan-doc helpers).
- `vizier-cli/src/jobs.rs` is a thin compatibility shim that re-exports the `vizier_core::jobs` API for existing CLI call sites.

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
- Fixture temp-root ownership and stale-cleanup policy also live in `tests/src/fixtures.rs` (`vizier-tests-build-*`, `vizier-tests-repo-*`, and legacy Vizier `.tmp*` roots).
- Per-workflow tests live in dedicated modules (`save.rs`, `draft.rs`, `approve.rs`, `review.rs`, `merge.rs`, `workspace.rs`, etc.).
- Scheduler/job coverage lives in `tests/src/background.rs` (scheduler flows, failure paths) and `tests/src/jobs.rs` (list/show/status/tail/attach/gc formatting and cleanup).
- `tests/src/lib.rs` is a thin module list that re-exports fixtures.
- Scheduler spec tests live under `vizier-kernel/src/scheduler/spec.rs` (pure kernel coverage).
- Integration tests run in parallel by default; set `VIZIER_TEST_SERIAL=1` to force the fixture lock in `tests/src/fixtures.rs` when debugging ordering-sensitive flakes.

## Conditional compilation
- The agent runner has unix-only execution paths plus a runtime mock path used during integration testing (`VIZIER_MOCK_AGENT=1` when the `integration_testing` feature is enabled).
