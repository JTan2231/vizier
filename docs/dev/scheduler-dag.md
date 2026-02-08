# Scheduler

## Scope
The scheduler runs all assistant-backed commands as background jobs. Each job is a node
in a DAG; edges are expressed as explicit job dependencies (`after`) and artifact
dependencies. The scheduler decides when a job is eligible to run, records wait
reasons, and spawns the job process.

For the full non-agent `.vizier/*` material contract (including jobs/build/sessions/sentinels
durability and compatibility notes), see `docs/dev/vizier-material-model.md`.

`vizier build execute` also uses scheduler jobs for build-session pipelines:
- internal `build_materialize` jobs materialize draft plan docs/branches
- existing `approve` / `review` / `merge` jobs execute per-step phases

## Architecture
- **Job records** live under `.vizier/jobs/<id>/`:
  - `job.json` is the canonical record.
  - `stdout.log` / `stderr.log` capture the child process streams.
  - `outcome.json` is written on finalization.
  - `command.patch` stores the scheduled command output patch (save jobs).
  - `save-input.patch` captures the input diff for scheduled save (save jobs only).
- **Scheduler core** lives in `vizier-cli/src/jobs.rs` (`scheduler_tick` and helpers).
- **CLI orchestration** enqueues jobs and advances the scheduler (see
  `vizier-cli/src/cli/dispatch.rs` and `vizier-cli/src/cli/scheduler.rs`).
- **Schedule metadata** is stored per job: `after`, `dependencies`, `locks`,
  `artifacts`, `pinned_head`, `approval`, `wait_reason`, and `waited_on`.
- **Scheduler lock** lives at `.vizier/jobs/scheduler.lock` and serializes scheduler
  ticks.

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
  `command.patch`, legacy `ask-save.patch`, and `save-input.patch`, and performs best-effort
  cleanup of owned temp worktrees when ownership/safety checks pass.
- Retry cleanup first attempts libgit2 prune and falls back to `git worktree remove --force <path>`
  plus `git worktree prune --expire now` when prune fails (including known `.git/shallow` stat
  failures).
- Retry clears `worktree_*` metadata only when cleanup is confirmed done/skipped; degraded cleanup
  retains `worktree_name`/`worktree_path`/`worktree_owned` and records
  `retry_cleanup_status`/`retry_cleanup_error` for later recovery via retry/cancel.
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
4) Approval  
5) Locks  
6) Spawn

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

## Human approval gate
`vizier approve <plan> --require-approval` records an approval gate in the job schedule:
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
- `wait_reason.kind` is one of `dependencies`, `pinned_head`, `approval`, or `locks` and includes
  a detail string describing the blocking condition.
- `waited_on` is a de-duplicated list of wait kinds the job has encountered over time.
- When a job becomes eligible to start, `wait_reason` is cleared.

## Artifact types
- `plan_branch` (draft branch exists)
- `plan_doc` (implementation plan file exists on the draft branch)
- `plan_commits` (draft branch exists)
- `target_branch` (target branch exists)
- `merge_sentinel` (merge conflict sentinel exists)
- `command_patch` (command patch file exists, or the referenced job finished `succeeded`; build-execute pipelines use this as a completion sentinel between phase jobs; legacy `ask_save_patch` records still deserialize)

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

These fields are also available in block/table formats via the list/show field
configuration (`display.lists.jobs` and `display.lists.jobs_show`).

## Scheduler schedule view (`vizier jobs schedule`)
`vizier jobs schedule` renders scheduler state in three formats:
- `summary` (default): one row per visible job for fast scanning.
- `dag`: verbose recursive dependency output for deep debugging.
- `json`: stable parseable contract for tooling.

Usage:
`vizier jobs schedule [--all] [--job <id>] [--format summary|dag|json] [--max-depth N]`

Summary behavior (default):
- Header: `Schedule (Summary)`.
- Columns: `#`, `Slug`, `Name`, `Status`, `Wait`, `Job`.
- Ordering is deterministic: `created_at ASC`, then `job_id ASC`.
- `--job <id>` focuses to the job neighborhood and pins the focused job to row 1.
- Default visibility includes active statuses plus `blocked_by_dependency`; `--all`
  additionally includes terminal statuses (`succeeded`, `failed`, `cancelled`).

DAG behavior (`--format dag`):
- Header: `Schedule (DAG, verbose)`.
- Preserves recursive dependency traversal for debugging:
  - `artifact -> job` edges with artifact leaf state (`[present]` / `[missing]`).
  - explicit `after:success -> <job-id> <status>` edges.
- `--max-depth` limits recursive expansion (default 3).

JSON behavior (`--format json` or global `--json`):
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
