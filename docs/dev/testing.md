# Testing guide

This repository keeps scheduler tests in three explicit layers so failures point to the right surface quickly without losing end-to-end coverage.

## Layers
- Rules (spec): Pure, deterministic scheduling decisions live in `vizier-kernel/src/scheduler/spec.rs`. Add unit tests here for dependency precedence, wait-reason ordering, and lock arbitration. These tests should not touch the filesystem or Git.
- Facts (extraction): Input collection happens in `vizier-cli/src/jobs.rs` (see `build_scheduler_facts`). Add focused tests here that validate artifact existence checks, producer discovery, pinned-head evaluation, and lock state collection. These tests can use temporary repos and job records but should avoid asserting final scheduling outcomes.
- Integration (effects/UX): CLI output, file effects, and end-to-end job flows stay in `tests/`. Keep formatting and side-effect assertions here.

## Adding new coverage
1. If the change is deterministic logic, add or update a spec test in `vizier-kernel/src/scheduler/spec.rs`.
2. If the change is about inputs (repo state, job records, locks, artifacts), add a fact extraction test in `vizier-cli/src/jobs.rs`.
3. If the change affects CLI output or side effects, add or update an integration test under `tests/`.

Kernel-only logic (config normalization, prompt assembly) should be covered with unit tests under `vizier-kernel/src/` so it stays pure and reusable across frontends.

Keep overlap minimal: rules should be validated in spec tests, and integration tests should focus on user-visible behavior.

## Fixture temp lifecycle
- Shared integration fixtures in `tests/src/fixtures.rs` own Vizier temp roots under the system temp dir.
- By default, fixture build roots are ephemeral for the current test process and stale Vizier-owned roots are cleaned up opportunistically before new fixture setup.
- Cleanup is intentionally scoped to Vizier-owned prefixes/markers (`vizier-tests-build-*`, `vizier-tests-repo-*`, and legacy `.tmp*` repos that match Vizier fixture markers) so unrelated temp directories are not touched.
- Set `VIZIER_TEST_KEEP_TEMP=1` when debugging to preserve fixture build roots across process exit.
