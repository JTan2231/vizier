# Jobs/read-only scheduler operations

Status (2026-02-20): ACTIVE. `vizier jobs` remains the retained scheduler/operator surface over persisted job records.

Thread: Jobs/read-only scheduler operations (cross: Scheduler docs, Reduced CLI surface stabilization)

Snapshot anchor
- Active threads — Jobs/read-only scheduler operations.
- Code state — Jobs list/show/schedule output contracts.

Tension
- Existing `jobs list/show --format json` output is display-oriented and string-flattens many scheduler fields.
- Operators need a stable typed contract for automation without changing default human-facing output.

Desired behavior (Product-level)
- Keep existing `vizier jobs` defaults/backward-compatible output unchanged.
- Add additive typed monitoring JSON on demand for list/show/schedule.
- Preserve scheduler semantics and schedule edge ordering/shape while improving machine-readable wait/schedule metadata.

Update (2026-02-20, raw monitoring JSON surface)
- `vizier jobs list`, `vizier jobs show`, and `vizier jobs schedule` now accept jobs-local `--raw` with explicit `--format json` only.
- Raw mode emits versioned typed envelopes (`version = 1`, `generated_at`) projected from persisted `JobRecord` data.
- `jobs schedule --format json --raw` keeps existing `edges` parity and deterministic ordering (`created_at_then_job_id`) while changing row `wait` to nullable typed `{kind, detail}`.
- Non-raw output behavior remains unchanged across block/table/json formats.
- Scheduler architecture docs now document raw and non-raw contracts side-by-side.

Update (2026-02-20, running-job PID liveness reconciliation)
- Scheduler ticks now reconcile stale `running` records before fact evaluation: missing/dead PID and identity mismatch states are terminalized as `failed` with additive `process_liveness_*` metadata.
- Stale workflow-node records preserve failed-route parity: reconciliation sets `workflow_node_outcome=failed` and reuses failed-route retry semantics with lock-safe sequencing.
- `vizier jobs tail --follow` and `vizier jobs attach` now run reconciliation while waiting, preventing indefinite hangs on stale `running` records.
- Scheduler docs and runtime/integration coverage were updated to reflect the new operator-visible failure behavior.

Pointers
- `vizier-core/src/jobs/mod.rs`
- `vizier-cli/src/cli/args.rs`
- `vizier-cli/src/cli/jobs_view.rs`
- `tests/src/jobs.rs`
- `docs/dev/scheduler-dag.md`
