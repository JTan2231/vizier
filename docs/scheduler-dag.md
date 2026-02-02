# Scheduler DAG semantics

## Overview
The background scheduler treats each job as a node in a DAG. Edges are expressed as
artifact dependencies: a job declares the artifacts it produces and the artifacts it
consumes. A job is eligible to run only when its dependencies, pinned head, and locks
are satisfied, in that order.

## Gate order
1) Dependencies
2) Pinned head
3) Locks
4) Spawn

## Dependency resolution
For each dependency (checked in order):
- If the artifact already exists, the dependency is satisfied regardless of producer
  status.
- If the artifact is missing and any producer is active (queued/waiting/running), the
  consumer waits with `waiting_on_deps` and a wait reason of `waiting on <artifact>`.
- If the artifact is missing and no producer is active:
  - If any producer succeeded, the consumer is blocked with `missing <artifact>`.
  - If all producers failed/cancelled/blocked, the consumer is blocked with
    `dependency failed for <artifact>`.

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
- `ask_save_patch` (ask/save patch file exists)

## Observability (jobs list/show)
The job list/show JSON output includes DAG-related fields so operators can inspect
scheduler state:
- `dependencies`
- `locks`
- `wait` (wait reason)
- `waited_on`
- `pinned_head`
- `artifacts`

These fields are also available in block/table formats via the list/show field
configuration (`display.lists.jobs` and `display.lists.jobs_show`).
