# Testing guide

This repository keeps scheduler tests in three explicit layers so failures point to the right surface quickly without losing end-to-end coverage.

## Layers
- Rules (spec): Pure, deterministic scheduling decisions live in `vizier-kernel/src/scheduler/spec.rs`. Add unit tests here for dependency precedence, wait-reason ordering, and lock arbitration. These tests should not touch the filesystem or Git.
- Facts (extraction): Input collection happens in `vizier-core/src/jobs/mod.rs` (see `build_scheduler_facts`). Add focused tests here that validate artifact existence checks, producer discovery, pinned-head evaluation, and lock state collection. These tests can use temporary repos and job records but should avoid asserting final scheduling outcomes.
- Integration (effects/UX): CLI output, file effects, and end-to-end job flows stay in `tests/`. Keep formatting and side-effect assertions here.
- Runtime bridge (node execution): Internal workflow runtime bridge coverage lives in `vizier-core/src/jobs/mod.rs` and should validate queue-time node materialization, `__workflow-node` dispatch contracts, outcome routing/retry behavior, custom prompt payload roundtrips (`custom:prompt_text:<key>` marker + payload store), and success/failure paths for every canonical executor operation/control policy.

## Adding new coverage
1. If the change is deterministic logic, add or update a spec test in `vizier-kernel/src/scheduler/spec.rs`.
2. If the change is about inputs (repo state, job records, locks, artifacts), add a fact extraction test in `vizier-core/src/jobs/mod.rs`.
3. If the change affects CLI output or side effects, add or update an integration test under `tests/`.

Kernel-only logic (config normalization, prompt assembly) should be covered with unit tests under `vizier-kernel/src/` so it stays pure and reusable across frontends.

Executor/control taxonomy logic should be covered in
`vizier-kernel/src/workflow_template.rs` with unit tests for:
- explicit executor-class classification (`environment.builtin`, `environment.shell`, `agent`)
- canonical prompt/invoke executor mapping (`cap.env.*.prompt.resolve` + `cap.agent.invoke`)
- prompt artifact contract enforcement (`custom:prompt_text:<key>` producer/consumer shape)
- control-node policy typing
- hard rejection of legacy `vizier.*` and legacy non-env `cap.*` labels
- rejection of unknown implicit `uses` labels

Keep overlap minimal: rules should be validated in spec tests, and integration tests should focus on user-visible behavior.

## Config migration matrix
Scope-decoupling work (alias/template identity) should keep integration coverage in three config modes:

- `tests/fixtures/config/legacy-scope-only.toml`: negative path; legacy `[agents.<scope>]` must fail with migration guidance.
- `tests/fixtures/config/alias-template-only.toml`: new `[commands]`, `[agents.commands.<alias>]`, and `[agents.templates."<selector>"]` path.
- `tests/fixtures/config/mixed-precedence.toml`: explicit precedence conflicts and CLI override precedence checks.

When touching config resolution or reporting:

- Update `tests/src/plan.rs` assertions for `vizier plan --json` under `commands.<alias>.*` fields.
- Ensure precedence checks cover: CLI override -> template override -> alias override -> default.
- Keep rejection coverage for unsupported dotted selectors and legacy config tables.

When touching scheduler metadata:

- Keep canonical metadata assertions (`metadata.command_alias`, `metadata.workflow_template_selector`, `metadata.execution_root`) and ensure legacy-only records fail where required by the runtime contract.
- Verify retry/status/show flows preserve alias/template metadata while clearing runtime-only fields.
- Verify `jobs show` prioritizes executor-first fields (`workflow_executor_class`, `workflow_executor_operation`, `workflow_control_policy`) and that historical records with legacy fields still deserialize safely.
- Include runtime metadata assertions for workflow-node jobs (`workflow_run_id`, `workflow_node_attempt`, `workflow_node_outcome`, `workflow_payload_refs`, `execution_root`) and ensure retry rewind clears outcome/payload while bumping node attempt counters.
- Add edge-propagation coverage for execution-root transitions (`worktree.prepare` propagation, active-target no-mutation guard, and `worktree.cleanup` reset to repo root).

## Fixture temp lifecycle
- Shared integration fixtures in `tests/src/fixtures.rs` own Vizier temp roots under the system temp dir.
- Integration fixtures cache a process-local template repository (seeded `.vizier` runtime surface, default `cicd.sh`, git init, agent shims) and clone from it per test instead of rebuilding repo scaffolding each time.
- Integration fixture setup seeds only the `.vizier` runtime surface required by tests (`config` + `narrative` plus empty plan/state dirs) and deliberately skips transient payloads like `.vizier/tmp/`, `.vizier/jobs/`, `.vizier/sessions/`, and `.vizier/tmp-worktrees/`.
- Integration fixtures build `vizier` once into the shared Cargo target directory (`$CARGO_TARGET_DIR` when set, otherwise `.vizier/tmp/cargo-target`) and stage a process-local fixture binary cache under the fixture build root.
- Per-test repos link (hard link/symlink when possible, copy fallback) to the staged fixture binary instead of copying a full binary payload every time.
- Integration fixtures prepend local `codex`/`gemini` backend stubs on `PATH` so tests cannot accidentally invoke paid external agent binaries even if a command resolves to the default shims.
- By default, fixture build roots are ephemeral for the current test process and stale Vizier-owned roots are cleaned up opportunistically before new fixture setup.
- Cleanup roots follow `env::temp_dir()` and, on macOS, also include `/private/tmp` to catch legacy roots created outside user-scoped temp dirs.
- Cleanup is intentionally scoped to Vizier-owned prefixes/markers (`vizier-tests-build-*`, `vizier-tests-repo-*`, legacy `.tmp*` repos that match Vizier fixture markers, and legacy `vizier-debug-*` roots that match the old `repo/` fixture layout) so unrelated temp directories are not touched.
- Fixture job polling defaults to 50ms; set `VIZIER_TEST_JOB_POLL_MS` to tune it for debugging noisy/slow environments.
- Set `VIZIER_TEST_KEEP_TEMP=1` when debugging to preserve fixture build roots across process exit.
- Set `VIZIER_TEST_SERIAL=1` to force fixture-level serialization when debugging ordering-sensitive integration flakes.
- Stage-run integration coverage in `tests/src/run.rs` uses `IntegrationRepo::new_serial()` so workflow DAG tests execute deterministically under default parallel `cargo test`.
