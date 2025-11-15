---
plan: the-unit-tests-need-to-be
branch: draft/the-unit-tests-need-to-be
status: implemented
created_at: 2025-11-15T09:22:29Z
spec_source: inline
implemented_at: 2025-11-15T09:49:14Z
---

## Operator Spec
the unit tests need to be able to run in parallel. they don't work without --test-threads=1. the integration tests under the tests/ binary also need to be able to run with cargo test

## Implementation Plan
## Overview
The current suite only passes when tests run serially and the integration “tests” crate acts like a bespoke runner that shells out to `cargo build` before invoking each scenario in sequence. That friction blocks developers (and agents) from running `cargo test` to validate changes, contradicting the repo’s emphasis on lean, auditable workflows in `.vizier/.snapshot`. We need to isolate per-test state so `vizier-core` unit tests can run concurrently, and reorganize the `tests` crate so its integration coverage executes under Cargo’s native harness.

## Execution Plan
1. **Baseline + Failure Mapping**
   - Run `cargo test` (without `--test-threads=1`) in the workspace to capture the current failures/timeouts; confirm the sources: `vizier-core/src/vcs.rs` tests rely on `std::env::set_current_dir`, and `tests/src/main.rs` depends on a shared `test-repo-active` plus a custom runner.
   - Log which functions still require `Repository::open(".")`/`discover(".")` so we know where to add path-aware variants.
   - Acceptance: reproducible failure notes and a checklist of APIs/tests that currently mutate global process state.

2. **Repo-Aware VCS Helpers**
   - Introduce internal helpers (e.g., `add_and_commit_in<P: AsRef<Path>>`, `stage_in`, `unstage_in`, `push_current_branch_in`) that accept an explicit repo/worktree path or `&Repository`. Keep the existing public functions as thin wrappers that call the new helpers with `std::env::current_dir()`/`Repository::discover(".")` so CLI behavior is unchanged.
   - Centralize shared setup (signature lookup, repo-state checks) to avoid code duplication and keep error messages identical.
   - Acceptance: new helpers exercised by unit tests, existing callers unaffected, and the API surface documented so future tests can avoid touching global CWD.

3. **Unit Test Isolation in `vizier-core`**
   - Replace `CwdGuard` with a dedicated `TestRepo` fixture that returns `{ tempdir, repo, path }`. File writes become `write(repo_path.join("…"), …)` instead of relying on the process CWD.
   - Update tests to call the new `*_in` helpers (e.g., `add_and_commit_in(&repo_path, …)`, `stage_in(&repo_path, …)`), and pass explicit paths to functions like `snapshot_staged`/`restore_staged`.
   - Remove any remaining `std::env::set_current_dir` usages from tests (including the `find_project_root` test; add a helper that takes a starting path).
   - Acceptance: `rg "set_current_dir" vizier-core/src -g '*tests*'` returns no hits, and `cargo test -p vizier-core` succeeds with default thread count.

4. **Modernize the `tests` Crate Harness**
   - Move the existing scenario functions into modules that expose `#[test]` cases; delete the manual `main`/`test!` macro.
   - Add a shared fixture (`struct IntegrationRepo`) that copies `test-repo` into a per-test `tempfile::TempDir`, configures Git user info, and exposes helpers to run `vizier` commands with `.current_dir(fixture.path())`. Each test owns its fixture so they can execute concurrently.
   - Provide a `vizier_binary()` helper backed by `OnceLock<PathBuf>` (or `once_cell::sync::Lazy`) that runs `cargo build --release --features mock_llm,integration_testing` exactly once per test process and caches `target/release/vizier`.
   - Update the `tests` crate `Cargo.toml` to include the new dependencies (`tempfile`, `once_cell`, maybe `assert_cmd` if useful) and keep the `git2` dependency.
   - Acceptance: running `cargo test -p tests` invokes each scenario via the standard harness, can run in parallel without directory conflicts, and still exercises the release binary with mock features.

5. **Command Matrix & CI Alignment**
   - Ensure workspace-level `cargo test` covers both `vizier-core` and the revamped `tests` crate without any `--test-threads` overrides.
   - Document (in the change description/commit) the commands developers should run locally (`cargo test`, `cargo test -p tests -- --nocapture` if needed) so future contributors know the expected flow.
   - Acceptance: CI (or local dry run) executes `cargo test` successfully; integration tests can be filtered/run individually (`cargo test -p tests test_save` etc.) thanks to the harness migration.

## Risks & Unknowns
- Nested `cargo build` invocations from the `vizier_binary()` helper could collide with whatever profile/features `cargo test` is already building; we may need to guard against simultaneous builds or detect when the binary is already present.
- Passing explicit repo paths through new helper functions must not regress runtime behavior (e.g., `push_current_branch_in` should still honor worktree detection akin to `Repository::discover(".")`).
- The `tests` crate now runs release builds during `cargo test`; this increases runtime and may need caching or feature gating if it becomes too slow.
- Additional dependencies (`once_cell`, `tempfile`) must be acceptable under the project’s dependency policy.

## Testing & Verification
- `cargo test -p vizier-core` (default parallelism) — proves the refactored unit tests no longer rely on serial execution or process-wide CWD.
- `cargo test -p vizier-cli` — sanity check to make sure CLI-focused tests still pass once helper APIs change signatures.
- `cargo test -p tests -- --nocapture` — exercises every integration scenario via the new harness; inspect logs to ensure each test uses its own temp fixture and the release binary path is reused.
- Workspace-wide `cargo test` — final confirmation that no crate requires `--test-threads=1` and that the new integration suite is wired into the default developer workflow.

## Notes
- Coordinate with anyone maintaining CI to remove any `RUST_TEST_THREADS=1` overrides or bespoke scripts once the new harness lands, so the benefit is visible immediately.
