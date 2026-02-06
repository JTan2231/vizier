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
  - `ask-save.patch` stores the ask/save output patch (ask/save jobs only).
  - `save-input.patch` captures the input diff for scheduled save (save jobs only).
- **Scheduler core** lives in `vizier-cli/src/jobs.rs` (`scheduler_tick` and helpers).
- **CLI orchestration** enqueues jobs and advances the scheduler (see
  `vizier-cli/src/cli/dispatch.rs` and `vizier-cli/src/cli/scheduler.rs`).
- **Schedule metadata** is stored per job: `after`, `dependencies`, `locks`,
  `artifacts`, `pinned_head`, `wait_reason`, and `waited_on`.
- **Scheduler lock** lives at `.vizier/jobs/scheduler.lock` and serializes scheduler
  ticks.

## Job lifecycle
Statuses:
- `queued`, `waiting_on_deps`, `waiting_on_locks`, `running` are active.
- `succeeded`, `failed`, `cancelled`, `blocked_by_dependency` are terminal.

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
  `ask-save.patch`, and `save-input.patch`, and attempts cleanup of owned
  temp worktrees when ownership/safety checks pass.
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
4) Locks  
5) Spawn

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

## Locks
Locks are shared or exclusive by key. When a lock cannot be acquired the job waits with:
- `status = waiting_on_locks`
- `wait_reason.kind = locks`
- `wait_reason.detail = "waiting on locks"`

## Wait reasons and waited_on
- `wait_reason.kind` is one of `dependencies`, `pinned_head`, or `locks` and includes
  a detail string describing the blocking condition.
- `waited_on` is a de-duplicated list of wait kinds the job has encountered over time.
- When a job becomes eligible to start, `wait_reason` is cleared.

## Artifact types
- `plan_branch` (draft branch exists)
- `plan_doc` (implementation plan file exists on the draft branch)
- `plan_commits` (draft branch exists)
- `target_branch` (target branch exists)
- `merge_sentinel` (merge conflict sentinel exists)
- `ask_save_patch` (ask/save patch file exists, or the referenced job finished `succeeded`; build-execute pipelines use this as a completion sentinel between phase jobs)

## Failure modes and exit codes
- `failed` is recorded when the background child exits with a non-zero code.
  Scheduler data errors (for example, a job missing `child_args`) are also marked
  failed and finalized with `exit_code = 1`.
- `cancelled` is operator-initiated (`vizier jobs cancel`) and uses exit code `143`.
- `blocked_by_dependency` is terminal; the scheduler will not retry it automatically
  (use `vizier jobs retry <job-id>` to rewind/requeue manually).
- `scheduler_tick` can return an error (for example, missing binary or record
  persistence failure). In those cases the job record remains queued until retried.
- `exit_code` is recorded on finalization; active or blocked jobs have no exit code.

## Observability (jobs list/show)
Job list/show output exposes scheduler fields so operators can inspect state:
- `after`
- `dependencies`
- `locks`
- `wait` (wait reason)
- `waited_on`
- `pinned_head`
- `artifacts`

These fields are also available in block/table formats via the list/show field
configuration (`display.lists.jobs` and `display.lists.jobs_show`).

## Scheduler DAG view (`vizier jobs schedule`)
`vizier jobs schedule` renders a read-only dependency graph so operators can see
what is waiting on what without drilling into individual job records.

Usage:
`vizier jobs schedule [--all] [--job <id>] [--format dag|json] [--max-depth N]`

Behavior:
- Default output is an ASCII DAG (no Unicode, no ANSI) with node lines:
  `<job-id> <status> [scope/plan/target] [wait: ...] [locks: ...] [pinned: ...]`.
- Dependencies render as `artifact -> job` edges; artifact leaves show `[present]`
  or `[missing]` when no producer exists.
- Explicit `after` edges render as `after:success -> <job-id> <status>`.
- `--all` includes succeeded/failed/cancelled jobs (default shows active +
  blocked_by_dependency).
- `--job` focuses on a single job and the producers/consumers around it.
- `--max-depth` limits dependency expansion (default 3).

JSON output (`--format json` or global `--json`) returns an adjacency list:
```
{
  "nodes": [
    { "id": "job-24", "status": "queued", "command": "vizier ask foo", "wait": "missing plan_doc:foo" }
  ],
  "edges": [
    { "from": "job-24", "to": "job-17", "after": { "policy": "success" } },
    { "from": "job-24", "to": "job-17", "artifact": "plan_doc:foo (draft/foo)" },
    { "from": "job-24", "to": "artifact:plan_branch:foo (draft/foo)", "artifact": "plan_branch:foo (draft/foo)", "state": "present" }
  ]
}
```

Empty state:
- If no matching jobs: stdout prints `Outcome: No scheduled jobs`.
- JSON format returns `{ "nodes": [], "edges": [] }`.

## GC safety
`vizier jobs gc` skips terminal records that are still referenced by any non-terminal
job’s `schedule.after` list, so cleanup cannot invalidate active `after` dependencies.
