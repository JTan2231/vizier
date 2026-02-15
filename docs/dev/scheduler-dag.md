# Scheduler

## Scope
The scheduler maintains and advances background job records for the retained
`vizier jobs` surface. Each job is a node in a DAG; edges are expressed as
explicit job dependencies (`after`) and artifact dependencies. The scheduler
decides when a job is eligible to run, records wait reasons, and spawns the
job process.

Stage orchestration is front-doored through `vizier run` aliases (for example
`draft`, `approve`, `merge`) that resolve repo-local templates under
`.vizier/workflows/*.toml` and materialize one scheduler job per template node.

For the full non-agent `.vizier/*` material contract (including jobs/build/sessions/sentinels
durability and compatibility notes), see `docs/dev/vizier-material-model.md`.

The kernel still carries template compilation/validation for scheduler metadata
and archival compatibility. Executor identity is now modeled explicitly as:
- `environment.builtin`
- `environment.shell`
- `agent`

Control behavior (`gate`, `retry`, terminal routing) is modeled separately and
is not an executor capability.

Canonical agent execution now uses one executor operation:
- `cap.agent.invoke` (`workflow_executor_operation = "agent.invoke"`).

Prompt construction is modeled as explicit upstream environment nodes:
- `cap.env.builtin.prompt.resolve`
- `cap.env.shell.prompt.resolve`

Prompt-resolve nodes produce exactly one custom artifact of shape
`custom:prompt_text:<key>`, and canonical `cap.agent.invoke` nodes consume
exactly one such artifact as input.

## Architecture
- **Job records** live under `.vizier/jobs/<id>/`:
  - `job.json` is the canonical record.
  - `stdout.log` / `stderr.log` capture the child process streams.
  - `outcome.json` is written on finalization.
  - `command.patch` stores runtime patch outputs (`git.save_worktree_patch` and
    patch-pipeline finalize flows).
  - `.vizier/jobs/runs/<run_id>.json` stores queue-time workflow runtime manifests for compiled template runs.
- **Scheduler core** lives in `vizier-core/src/jobs/mod.rs` (`scheduler_tick` and helpers).
- **CLI jobs module** (`vizier-cli/src/jobs.rs`) is a compatibility re-export shim over `vizier_core::jobs`.
- **CLI orchestration** renders/operates scheduler state through
  `vizier-cli/src/cli/jobs_view.rs`.
- **Schedule metadata** is stored per job: `after`, `dependencies`, `locks`,
  `artifacts`, `pinned_head`, `approval`, `wait_reason`, and `waited_on`.
- **Workflow-template compile metadata** is stored per job in `metadata`:
  `workflow_run_id`,
  `workflow_node_attempt`,
  `workflow_node_outcome`,
  `workflow_payload_refs`,
  `workflow_template_id`, `workflow_template_version`, `workflow_node_id`,
  `workflow_executor_class`, `workflow_executor_operation`,
  `workflow_control_policy`,
  `workflow_policy_snapshot_hash`, and `workflow_gates`.
- **Workflow runtime entrypoint** is an internal hidden command:
  `vizier __workflow-node --job-id <id>`. Scheduler jobs materialized from
  template nodes execute through this entrypoint and are intentionally excluded
  from help/man/completion surfaces.
- **Workflow-template compile validation** rejects jobs that reference undeclared
  artifact contracts, unknown template `after` nodes, or invalid `on.<outcome>`
  multiplexers before enqueue.
- **Scheduler lock** lives at `.vizier/jobs/scheduler.lock` and serializes scheduler
  ticks.

## Canonical `uses` Contract
- Workflow template `uses` labels are canonical-only.
- Accepted families:
  - executor IDs: `cap.env.*` and `cap.agent.invoke`
  - control IDs: `control.*`
- Legacy `vizier.*` labels and legacy non-env `cap.*` labels fail template
  validation immediately.
- Unknown arbitrary `uses` labels are rejected; there is no implicit fallback
  to executable custom-command capability.

## Runtime operation coverage
`__workflow-node` runtime dispatch now executes the full canonical operation and
policy inventory accepted by the template validator.

Executor operations:
- `prompt.resolve`
- `agent.invoke`
- `worktree.prepare`
- `worktree.cleanup`
- `plan.persist`
- `git.stage_commit`
- `git.integrate_plan_branch`
- `git.save_worktree_patch`
- `patch.pipeline_prepare`
- `patch.pipeline_finalize`
- `patch.execute_pipeline`
- `build.materialize_step`
- `merge.sentinel.write`
- `merge.sentinel.clear`
- `command.run`
- `cicd.run`

Control policies:
- `gate.stop_condition`
- `gate.conflict_resolution`
- `gate.cicd`
- `gate.approval`
- `terminal`

Runtime notes:
- handlers resolve execution root in metadata precedence order:
  `metadata.execution_root` -> repo root, and reject out-of-repo paths.
- `agent.invoke` uses resolved configured runner settings (no prompt-echo
  facade path).
- `git.integrate_plan_branch` now embeds the loaded plan document markdown in
  the merge commit message and commits source-branch plan-doc removal before
  merge finalization when a plan slug is available.
- `terminal` is an explicit sink policy and fails when outgoing routes are
  configured.
- conflict/cicd/approval gates route as `succeeded`/`failed`/`blocked` outcomes
  for scheduler retry/edge handling.
- `on.succeeded` edges remain materialized as `after:success` dependencies for
  scheduler determinism, with runtime metadata propagation occurring along those
  explicit success edges.

## Job lifecycle
Statuses:
- `queued`, `waiting_on_deps`, `waiting_on_approval`, `waiting_on_locks`, `running` are active.
- `succeeded`, `failed`, `cancelled`, `blocked_by_dependency`, `blocked_by_approval` are terminal.

Terminal jobs are never re-run by the scheduler. `blocked_by_dependency` indicates a
dependency can no longer be satisfied (see below).

## Retry (`vizier jobs retry <job-id>`)
Use retry to rewind a failed/blocked segment and re-queue it without editing
job JSON by hand.

Internal contract:
- **retry root**: the requested job id.
- **last successful point**: direct predecessors of the root that are currently
  `succeeded`.
- **retry set**: retry root plus all downstream dependents reachable via
  `after` edges and produced-artifact consumer edges.
- **predecessors are untouched**: upstream jobs are not rewound.

Safety and rewind behavior:
- Retry fails fast if any job in the retry set is currently `running`.
- `queued`/`waiting_on_*` jobs inside the retry set are rewound and re-queued.
- For each job in the retry set, retry clears runtime state:
  `status=queued`, `pid`, `started_at`, `finished_at`, `exit_code`,
  `session_path`, `outcome_path`, `schedule.wait_reason`, and
  `schedule.waited_on`.
- Retry truncates `stdout.log`/`stderr.log`, removes stale `outcome.json`,
  `command.patch`, and custom-artifact markers owned by rewound jobs, and performs best-effort
  cleanup of owned temp worktrees when ownership/safety checks pass.
- Retry cleanup first attempts libgit2 prune and falls back to `git worktree remove --force <path>`
  plus `git worktree prune --expire now` when prune fails (including known `.git/shallow` stat
  failures).
- Retry clears `worktree_*` metadata only when cleanup is confirmed done/skipped; degraded cleanup
  retains worktree ownership metadata and records
  `retry_cleanup_status`/`retry_cleanup_error` for later recovery via retry/cancel.
- When retry cleanup is done/skipped, `metadata.execution_root` resets to `.`
  (repo-root marker). Degraded cleanup preserves `execution_root`.
- Merge-related retry sets also clear scheduler-owned conflict sentinels under
  `.vizier/tmp/merge-conflicts/<slug>.json`. If Git is currently in an
  in-progress merge/cherry-pick state, retry fails with guidance instead of
  mutating conflict state.

After rewind, retry immediately runs one scheduler tick and reports which jobs
were reset and which were restarted.

## Gate order
1) `after` dependencies  
2) Artifact dependencies  
3) Pinned head  
4) Preconditions  
5) Approval  
6) Locks  
7) Spawn

## Explicit `after` dependency resolution
`after` dependencies are explicit job-id constraints (`--after <job-id>`) and are
checked before artifact dependencies.

Policy today:
- `success` (default and only policy): predecessor must finish with `succeeded`.

Resolution:
- Missing predecessor record: block with `missing job dependency <job-id>`.
- Active predecessor (`queued` / `waiting_on_deps` / `waiting_on_locks` / `running`):
  wait with `waiting_on_deps` and detail `waiting on job <job-id>`.
- `succeeded`: dependency satisfied.
- Terminal non-success (`failed` / `cancelled` / `blocked_by_dependency`): block with
  `dependency failed for job <job-id> (<status>)`.
- Invalid/unreadable predecessor data: block with
  `scheduler data error for job dependency <job-id>: <error>`.

## Artifact dependency resolution
Dependencies are checked in order. For each dependency:
- If the artifact already exists, the dependency is satisfied regardless of producer
  status.
  For built-in artifact kinds this uses repository/job-state probes. Custom artifacts
  use persisted scheduler markers under `.vizier/jobs/artifacts/custom/...`.
  Prompt-text custom artifacts can additionally persist typed payload JSON under
  `.vizier/jobs/artifacts/data/<type_hex>/<key_hex>/<job_id>.json`; marker
  existence remains the scheduler readiness source of truth.
- If the artifact is missing and any producer is active (queued/waiting/running), the
  consumer waits with `waiting_on_deps` and a wait reason of `waiting on <artifact>`.
- If the artifact is missing and no producer is active:
  - If there are no producers for the artifact, the consumer is blocked with
    `missing <artifact>`.
  - If any producer succeeded, the consumer is blocked with `missing <artifact>`.
  - If all producers failed/cancelled/blocked, the consumer is blocked with
    `dependency failed for <artifact>`.

## Pinned head behavior
If a job has a `pinned_head` and the repo head no longer matches, the job waits with:
- `status = waiting_on_deps`
- `wait_reason.kind = pinned_head`
- `wait_reason.detail = "pinned head mismatch on <branch>"`

## Scheduler preconditions
`schedule.preconditions` supports additional readiness gates independent of artifact or lock checks.
- `clean_worktree`: waits until tracked/untracked changes are cleared (ephemeral `.vizier/jobs|sessions|tmp|tmp-worktrees` paths are ignored).
- `branch_exists`: waits until the required local branch exists (branch may come from explicit precondition args, `pinned_head.branch`, or a single `branch:*` lock key).
- `custom`: currently supports `clean_worktree` and `branch_exists`; unknown custom preconditions block with a descriptive error detail.

When unsatisfied, jobs stay in `waiting_on_deps` (`wait_reason.kind = preconditions`).
When a precondition is invalid/unresolvable, jobs are marked `blocked_by_dependency` with `wait_reason.kind = preconditions`.

## Human approval gate
Any queued job can carry an approval gate in schedule metadata:
- `schedule.approval.required = true`
- `schedule.approval.state = pending`
- `schedule.approval.requested_at/requested_by` capture who queued the gate

Decision commands:
- `vizier jobs approve <job-id>` transitions `pending -> approved`, stamps `decided_at/decided_by`, then runs one scheduler tick.
- `vizier jobs reject <job-id> [--reason TEXT]` transitions `pending -> rejected`, records the reason, and finalizes the job as `blocked_by_approval`.

Scheduler behavior:
- `approval.state = pending` => `status = waiting_on_approval`, `wait_reason.kind = approval`, detail `awaiting human approval`.
- `approval.state = approved` => job continues to lock checks/spawn.
- `approval.state = rejected` => terminal `blocked_by_approval` (with reason when provided).

## Locks
Locks are shared or exclusive by key. When a lock cannot be acquired the job waits with:
- `status = waiting_on_locks`
- `wait_reason.kind = locks`
- `wait_reason.detail = "waiting on locks"`

## Wait reasons and waited_on
- `wait_reason.kind` is one of `dependencies`, `pinned_head`, `preconditions`, `approval`, or `locks` and includes
  a detail string describing the blocking condition.
- `waited_on` is a de-duplicated list of wait kinds the job has encountered over time.
- When a job becomes eligible to start, `wait_reason` is cleared.

## Artifact types
- `plan_branch` (draft branch exists)
- `plan_doc` (implementation plan file exists on the draft branch)
- `plan_commits` (draft branch exists)
- `target_branch` (target branch exists)
- `merge_sentinel` (merge conflict sentinel exists)
- `command_patch` (command patch file exists, or the referenced job finished `succeeded`; historical build-execute pipelines used this as a phase sentinel)
- `custom` (`custom:<type_id>:<key>`; extension artifact kind for template-defined producer/consumer wiring)

## Failure modes and exit codes
- `failed` is recorded when the background child exits with a non-zero code.
  Scheduler data errors (for example, a job missing `child_args`) are also marked
  failed and finalized with `exit_code = 1`.
- `cancelled` is operator-initiated (`vizier jobs cancel`) and uses exit code `143`.
- `blocked_by_dependency` is terminal; the scheduler will not retry it automatically
  (use `vizier jobs retry <job-id>` to rewind/requeue manually).
- `blocked_by_approval` is terminal and indicates a human rejected execution.
- `scheduler_tick` can return an error (for example, missing binary or record
  persistence failure). In those cases the job record remains queued until retried.
- `exit_code` is recorded on finalization; active or blocked jobs have no exit code.

## Observability (jobs list/show)
Job list/show output exposes scheduler fields so operators can inspect state:
- `after`
- `dependencies`
- `locks`
- `approval_required`, `approval_state`, `approval_decided_by`
- `wait` (wait reason)
- `waited_on`
- `pinned_head`
- `artifacts`
- `workflow template` / `workflow node` / `workflow policy snapshot` / `workflow gates`
- `workflow capability` (legacy) / `workflow executor class` / `workflow executor operation` / `workflow control policy`
- `execution_root` (effective runtime filesystem root marker)

These fields are also available in block/table formats via the list/show field
configuration (`display.lists.jobs` and `display.lists.jobs_show`).

## Scheduler schedule view (`vizier jobs schedule`)
`vizier jobs schedule` renders scheduler state in three formats:
- `summary` (default): one row per visible job for fast scanning.
- `dag`: verbose recursive dependency output for deep debugging.
- `json`: stable parseable contract for tooling.

Usage:
`vizier jobs schedule [--all] [--job <id>] [--format summary|dag|json] [--watch] [--top N] [--interval-ms MS] [--max-depth N]`

Summary behavior (default):
- Header: `Schedule (Summary)`.
- Columns: `#`, `Slug`, `Name`, `Status`, `Wait`, `Job`.
- Ordering is deterministic: `created_at ASC`, then `job_id ASC`.
- `--job <id>` focuses to the job neighborhood and pins the focused job to row 1.
- Default visibility includes active statuses plus failed jobs that are currently blocking dependency-waiting jobs; `--all`
  additionally includes terminal statuses (`succeeded`, `failed`, `cancelled`).

DAG behavior (`--format dag`):
- Header: `Schedule (DAG, verbose)`.
- Preserves recursive dependency traversal for debugging:
  - `artifact -> job` edges with artifact leaf state (`[present]` / `[missing]`).
  - explicit `after:success -> <job-id> <status>` edges.
- `--max-depth` limits recursive expansion (default 3).

JSON behavior (`--format json`):
- Top-level contract:
  - `version` (currently `1`)
  - `ordering` (`"created_at_then_job_id"`)
  - `jobs` (ordered rows matching summary order)
  - `edges` (dependency edges for the visible schedule view)
- Each `jobs[]` entry includes:
  - `order`
  - `job_id`
  - `slug` (nullable)
  - `name`
  - `status`
  - `wait` (nullable)
  - `created_at` (RFC3339)
- Each `edges[]` entry includes:
  - `from`
  - `to`
  - either `after` (`{ policy }`) or `artifact` (with optional `state`)

Watch behavior (`--watch`):
- Interactive summary dashboard with in-place ANSI redraw (top-style) that includes:
  - refresh header (timestamp + poll interval)
  - status bucket counts (`queued`, `waiting`, `running`, `blocked`, `terminal`)
  - top-N summary table (`--top`, default `10`, minimum `1`)
  - running-job pane showing the selected job and latest `[stdout]`/`[stderr]` line
- Workflow node runtime errors are finalized as terminal `failed` jobs (exit `1`) so watch does not leave those nodes in a stale `running` state.
- Running job selection:
  - if `--job <id>` is set and that job is `running`, watch pins to that job
  - otherwise watch uses the first visible `running` summary row (after top-N truncation)
- Latest-line source reads bounded log tails from `stdout.log` and `stderr.log` and picks the most recently modified stream.
- Guardrails:
  - requires interactive stdout/stderr TTY with ANSI enabled
  - `--watch` rejects `--format dag|json` (summary-only mode)
  - `--interval-ms` default `500`, minimum `100`

Example:
```
{
  "version": 1,
  "ordering": "created_at_then_job_id",
  "jobs": [
    {
      "order": 1,
      "job_id": "job-24",
      "slug": "foo",
      "name": "approve/foo/main",
      "status": "queued",
      "wait": "waiting on dependency",
      "created_at": "2026-02-01T12:00:00+00:00"
    }
  ],
  "edges": [
    { "from": "job-24", "to": "job-17", "after": { "policy": "success" } },
    { "from": "job-24", "to": "job-17", "artifact": "plan_doc:foo (draft/foo)" }
  ]
}
```

Empty state:
- `summary` and `dag`: stdout prints `Outcome: No scheduled jobs`.
- `json`:
  ```
  {
    "version": 1,
    "ordering": "created_at_then_job_id",
    "jobs": [],
    "edges": []
  }
  ```

## GC safety
`vizier jobs gc` skips terminal records that are still referenced by any non-terminal
job’s `schedule.after` list, so cleanup cannot invalidate active `after` dependencies.
