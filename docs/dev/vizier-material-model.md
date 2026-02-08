# Vizier-Managed Material Model (Non-Agent)

This document is the canonical contract for Vizier-managed repository material that is not agent-runtime internals.
It describes what exists under `.vizier/`, which flows own it, how state is represented, and how recovery works.

## Scope

In scope:
- Workflow/scheduler/auditor artifacts that Vizier creates, updates, or consumes.
- User-visible material for `draft/approve/review/merge/build/jobs/save`.

Out of scope:
- Agent runtime internals (shim protocol details, provider request/response payloads, backend transport mechanics).

## Canonical Entities

### 1) Narrative material
- Paths: `.vizier/narrative/**/*.md`, plus legacy `.vizier/.snapshot` compatibility handling.
- Primary files: `.vizier/narrative/snapshot.md`, `.vizier/narrative/glossary.md`, and thread docs under `.vizier/narrative/threads/`.
- Owner flows: save/approve/review/merge refresh flows.
- Durability: durable repository material.

### 2) Plan material
- Path: `.vizier/implementation-plans/<slug>.md`.
- Branch affinity: `draft/<slug>` by default.
- Required front matter keys:
  - `plan: <slug>`
  - `branch: <draft-branch>`
- Required sections:
  - `## Operator Spec`
  - `## Implementation Plan`
- Owner flows: `vizier draft`, build materialization, `vizier approve`, `vizier merge`.
- Durability: durable during plan workflow; removed from target branch during merge completion.

### 3) Build session material
- Root: `.vizier/implementation-plans/builds/<build_id>/`.
- Artifacts:
  - `manifest.json`
  - `summary.md`
  - `plans/*.md`
  - copied build input under `input/`
- Branch affinity: `build/<build_id>`.
- Owner flows: `vizier build` create mode.
- Durability: durable workflow artifact.

### 4) Build execution material
- Path: `.vizier/implementation-plans/builds/<build_id>/execution.json`.
- Fields (contract-critical):
  - `build_id`
  - `pipeline_override`
  - `stage_barrier`
  - `failure_mode`
  - `created_at`
  - `status`
  - per-step `derived_slug`, `derived_branch`, policy snapshot, and phase job ids.
- Owner flows: `vizier build execute`.
- Durability: durable workflow state used for resume/reuse/mismatch detection.

### 5) Scheduler job material
- Root: `.vizier/jobs/<job_id>/`.
- Per-job artifacts:
  - `job.json` (canonical record)
  - `stdout.log`
  - `stderr.log`
  - `outcome.json` (written on finalization)
  - optional `command.patch` (legacy `ask-save.patch` still recognized for retries) and `save-input.patch`
- Schedule metadata in `job.json`:
  - `after`
  - `dependencies`
  - `locks`
  - `artifacts`
  - `pinned_head`
  - `approval` (`required`, `state`, request/decision metadata, optional reason)
  - `wait_reason`
  - `waited_on`
- Owner flows: all scheduler-backed commands.
- Durability: scheduler-durable operational material (subject to `vizier jobs gc` policy).

### 6) Merge conflict sentinel material
- Path: `.vizier/tmp/merge-conflicts/<slug>.json`.
- Purpose: stores merge/cherry-pick resume context for `vizier merge --complete-conflict`.
- Owner flows: `vizier merge` conflict handling and completion.
- Durability: ephemeral operational artifact; removed on successful completion.

### 7) Session log material
- Path: `.vizier/sessions/<session_id>/session.json`.
- Schema marker: `vizier.session.v1`.
- Contains: transcript/messages, effective config/prompt/model snapshot, operation data, and outcome summary.
- Owner flows: Auditor session logging for assistant-backed operations.
- Durability: durable audit artifact.

### 8) Temp worktree material
- Root: `.vizier/tmp-worktrees/`.
- Purpose: disposable isolation for draft/approve/review/merge/build materialization and scheduler jobs.
- Owner flows: worktree-backed command execution.
- Durability: ephemeral operational artifact (may be intentionally preserved on failure for recovery).

## Relationship Model

- `Plan(slug)` and `draft/<slug>` are intended as a 1:1 workflow identity pair.
- `BuildSession(build_id)` owns manifest steps and build outputs under `build/<build_id>`.
- `BuildExecutionStep(step_key)` derives `derived_slug` and `derived_branch`, then points at phase jobs.
- `JobRecord` edges form a DAG through:
  - explicit `after` dependencies (job-id level), and
  - artifact dependencies (`plan_branch`, `plan_doc`, `plan_commits`, `target_branch`, `merge_sentinel`, `command_patch`).
- `MergeConflictSentinel(slug)` links pending merge state to plan slug, source branch, target branch, and resume metadata.
- `SessionLog(session_id)` is attached to audited operations and referenced by outcomes/commits.

## State Models

### Scheduler job status (`JobStatus`)

Active:
- `queued`
- `waiting_on_deps`
- `waiting_on_approval`
- `waiting_on_locks`
- `running`

Terminal:
- `succeeded`
- `failed`
- `cancelled`
- `blocked_by_dependency`
- `blocked_by_approval`

### Build execution status (`BuildExecutionStatus`)
- `queued`
- `running`
- `succeeded`
- `failed`
- `cancelled`

### Build manifest step result (`ManifestStepResult`)
- `pending`
- `succeeded`
- `failed`

## Durability Classes

Durable workflow artifacts:
- `.vizier/narrative/**/*`
- `.vizier/implementation-plans/*.md` (workflow durable)
- `.vizier/implementation-plans/builds/<build_id>/*`
- `.vizier/sessions/<session_id>/session.json`

Scheduler-durable operational artifacts:
- `.vizier/jobs/<job_id>/*`

Ephemeral operational artifacts:
- `.vizier/tmp/*`
- `.vizier/tmp-worktrees/*`
- `.vizier/tmp/merge-conflicts/<slug>.json` (until merge completion)

## Invariants

- Plan slug normalization is lowercase, dash-separated.
- Default plan branch naming is `draft/<slug>`.
- Workflow-consumed plan docs must include front matter keys `plan` and `branch`.
- Build execute resume rejects execution policy mismatch against existing `execution.json`.
- Scheduler retry refuses to mutate running jobs; it rewinds queued/waiting descendants and preserves predecessor success boundaries.
- `vizier merge --complete-conflict` only operates on existing Vizier-managed sentinel state.
- Clean-worktree checks ignore ephemeral Vizier paths:
  - `.vizier/jobs`
  - `.vizier/sessions`
  - `.vizier/tmp`
  - `.vizier/tmp-worktrees`

## Recovery Semantics

### Scheduler retry (`vizier jobs retry <job-id>`)
- Computes retry set (root + downstream dependents).
- Rewinds runtime fields and scheduler-owned operational artifacts.
- Requeues rewound jobs and advances scheduler.
- Refuses retry if any job in the retry set is currently running.

### Merge conflict completion
- Sentinel state preserves merge/cherry-pick context required to finish.
- Manual or agent-assisted conflict resolution resumes from sentinel metadata.
- Successful completion clears sentinel state.

## Compatibility and Reconciliation

### Legacy snapshot alias vs snapshot-first narrative posture
- Snapshot-first posture treats `.vizier/narrative/snapshot.md` as canonical.
- Compatibility still exists for legacy `.vizier/.snapshot` discovery/path checks in core utilities and file tracking.
- Operational guidance: new updates should target `.vizier/narrative/snapshot.md`; legacy alias exists for backward compatibility, not as preferred storage.

### Plan doc durability vs merge-time plan removal
- Plan docs are durable workflow artifacts while draft/build/review/approval work is active.
- `vizier merge` intentionally removes `.vizier/implementation-plans/<slug>.md` from the merged target history after embedding plan context into merge commit metadata/body.
- Reconciliation: workflow durability does not imply target-branch permanence.

### Plan-doc/branch identity vs current inventory drift
- Intended contract: one plan slug maps to one `draft/<slug>` identity.
- Current repositories may show drift between plan docs and local draft branches; pending-plan surfaces can become less trustworthy in that state.
- Reconciliation: treat 1:1 as the target contract and keep transitional operator guidance until hygiene enforcement lands.

## Source Anchors

- Plan schema/front matter: `vizier-cli/src/plan.rs`
- Build manifests/execution state: `vizier-cli/src/actions/build.rs`
- Job records/scheduler metadata: `vizier-cli/src/jobs.rs`, `vizier-kernel/src/scheduler/mod.rs`, `docs/dev/scheduler-dag.md`
- Merge conflict sentinel lifecycle: `vizier-cli/src/actions/merge.rs`
- Session schema/path: `vizier-core/src/auditor.rs`
- Clean-worktree exclusions: `vizier-core/src/vcs/status.rs`
