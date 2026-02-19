# Vizier-Managed Material Model (Non-Agent)

This document is the canonical contract for Vizier-managed repository material that is not agent-runtime internals.
It describes what exists under `.vizier/`, which flows own it, how state is represented, and how recovery works.

## Scope

In scope:
- Workflow/scheduler/auditor artifacts that Vizier creates, updates, or consumes.
- User-visible material for retained commands (`init`, `list`, `cd`, `clean`, `jobs`, `release`) plus canonical workflow artifacts.

Out of scope:
- Agent runtime internals (shim protocol details, provider request/response payloads, backend transport mechanics).

## Canonical Entities

### 1) Narrative material
- Paths: `.vizier/narrative/**/*.md`.
- Primary files: `.vizier/narrative/snapshot.md`, `.vizier/narrative/glossary.md`, and thread docs under `.vizier/narrative/threads/`.
- Owner flows: narrative upkeep, repository initialization bootstrap, and documentation maintenance.
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
- Owner flows: stage templates executed through `vizier run draft|approve|merge` plus retained plan visibility surfaces (`vizier list`, `vizier cd`, `vizier clean`).
- Durability: durable repository artifact; plan/branch bijection can drift in legacy worktrees.

### 2a) Stage template material
- Paths:
  - `.vizier/workflows/draft.hcl`
  - `.vizier/workflows/approve.hcl`
  - `.vizier/workflows/merge.hcl`
  - optional composed flows such as `.vizier/develop.hcl`
- Owner flows: `vizier run <alias|selector>` queue-time template resolution and compilation.
- Durability: durable repository orchestration contract.

### 3) Build session material
- Root: `.vizier/implementation-plans/builds/<build_id>/`.
- Artifacts:
  - `manifest.json`
  - `summary.md`
  - `plans/*.md`
  - copied build input under `input/`
- Branch affinity: `build/<build_id>`.
- Owner flows: historical build workflows (command family removed; artifacts may remain).
- Durability: durable workflow artifact.

### 4) Build execution material
- Path: `.vizier/implementation-plans/builds/<build_id>/execution.<resume-key>.json` (`resume-key=default` uses `execution.json`).
- Fields (contract-critical):
  - `build_id`
  - `pipeline_override`
  - `stage_barrier`
  - `failure_mode`
  - `template_id` / `template_version`
  - `resume_key` / `resume_reuse_mode`
  - `policy_snapshot_hash`
  - `policy_snapshot` (normalized template/policy snapshot used for resume drift checks)
  - `created_at`
  - `status`
  - per-step `derived_slug`, `derived_branch`, policy snapshot, and node/phase job ids.
- Owner flows: historical build execution workflows (command family removed; state files may remain for audit/recovery).
- Durability: durable workflow state used for resume/reuse/mismatch detection.

### 5) Scheduler job material
- Root: `.vizier/jobs/<job_id>/`.
- Per-job artifacts:
  - `job.json` (canonical record)
  - `stdout.log`
  - `stderr.log`
  - `outcome.json` (written on finalization)
  - optional `command.patch`
- Workflow-run manifests:
  - `.vizier/jobs/runs/<workflow_run_id>.json`
  - captures compiled node runtime metadata (`job_id`, `uses`, args, outcome routing, retry policy, per-outcome artifact sets) for internal `__workflow-node` execution.
- Schedule metadata in `job.json`:
  - `after`
  - `dependencies`
  - `locks`
  - `artifacts`
  - `pinned_head`
  - `approval` (`required`, `state`, request/decision metadata, optional reason)
  - `wait_reason`
  - `waited_on`
- Workflow compile metadata in `job.json.metadata`:
  - `workflow_run_id`
  - `workflow_node_attempt`
  - `workflow_node_outcome`
  - `workflow_payload_refs`
  - `workflow_template_id`
  - `workflow_template_version`
  - `workflow_node_id`
  - `workflow_executor_class`
  - `workflow_executor_operation`
  - `workflow_control_policy`
  - `workflow_policy_snapshot_hash`
  - `workflow_gates`
  - `execution_root` (logical runtime root marker; `.` means repo root)
  - `worktree_name` / `worktree_owned` (worktree lifecycle ownership/context)
- Owner flows: all scheduler-backed commands.
- Durability: scheduler-durable operational material (subject to `vizier jobs gc` policy).

Custom artifact payload adjunct:
- Prompt artifact markers under `.vizier/jobs/artifacts/custom/...` are still the scheduler gating contract.
- Optional typed payload data for custom artifacts is stored under
  `.vizier/jobs/artifacts/data/<type_hex>/<key_hex>/<job_id>.json`.

Compatibility policy:
- Workflow template identity is canonical-only (`cap.env.*`, `cap.agent.invoke`,
  and `control.*`).
- Legacy `vizier.*` labels and legacy non-env `cap.*` labels are rejected
  during template validation.
- Unknown arbitrary `uses` labels are rejected; there is no implicit fallback to
  executable custom capability.

### 6) Merge conflict sentinel material
- Path: `.vizier/tmp/merge-conflicts/<slug>.json`.
- Purpose: stores historical merge/cherry-pick resume context from removed workflow commands.
- Owner flows: legacy merge conflict handling state only.
- Durability: ephemeral operational artifact; removed on successful completion.

### 7) Session log material
- Path: `.vizier/sessions/<session_id>/session.json`.
- Schema marker: `vizier.session.v1`.
- Contains: transcript/messages, effective config/prompt/model snapshot, operation data, and outcome summary.
- Owner flows: Auditor session logging for assistant-backed operations.
- Durability: durable audit artifact.

### 8) Temp worktree material
- Root: `.vizier/tmp-worktrees/`.
- Purpose: disposable isolation for retained workspace/scheduler operations plus legacy workflow residue.
- Owner flows: worktree-backed command execution (`vizier cd`, `vizier clean`, scheduler cleanup/retry paths).
- Durability: ephemeral operational artifact (may be intentionally preserved on failure for recovery).

## Relationship Model

- `Plan(slug)` and `draft/<slug>` are intended as a 1:1 workflow identity pair.
- `BuildSession(build_id)` owns manifest steps and build outputs under `build/<build_id>`.
- `BuildExecutionStep(step_key)` derives `derived_slug` and `derived_branch`, then points at phase jobs.
- `JobRecord` edges form a DAG through:
  - explicit `after` dependencies (job-id level), and
  - artifact dependencies (`plan_branch`, `plan_doc`, `plan_commits`, `target_branch`, `merge_sentinel`, `command_patch`, plus `custom:<type_id>:<key>` extension artifacts).
  Readiness is additionally gated by optional `pinned_head`, declarative `preconditions`, approval facts, and lock acquisition.
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
- Build execute resume reuses the execution state lane selected by template `policy.resume.key` and enforces drift according to `policy.resume.reuse_mode` (`strict` rejects all drift; `compatible` still rejects node/edge/artifact drift but allows policy-only drift).
- Canonical agent execution nodes use `cap.agent.invoke`, consume exactly one prompt artifact dependency (`custom:prompt_text:<key>`), and do not source runtime command/script args from node config.
- Prompt-resolve nodes (`cap.env.builtin.prompt.resolve` / `cap.env.shell.prompt.resolve`) produce exactly one prompt artifact (`custom:prompt_text:<key>`); shell prompt resolvers require exactly one of `args.command` or `args.script`.
- Scheduler retry refuses to mutate running jobs; it rewinds queued/waiting descendants and preserves predecessor success boundaries.
- Runtime node dispatch (`__workflow-node`) executes the complete canonical
  operation/policy inventory accepted by template validation (all
  `cap.env.*`/`cap.agent.invoke` executor operations and `control.*` policies
  mapped in `vizier-kernel/src/workflow_template.rs`).
- Worktree lifecycle artifacts are ownership-bound:
  - only job-owned worktrees recorded in metadata are eligible for automatic
    cleanup;
  - degraded cleanup keeps worktree metadata for subsequent retry/cancel
    recovery.
- Runtime execution-root propagation is edge-local:
  - `worktree.prepare` sets both `worktree_*` ownership metadata and
    `execution_root`.
  - runtime success edges propagate execution context to downstream nodes.
  - `worktree.cleanup` success resets `execution_root` to `.` and clears
    worktree ownership metadata.
- Terminal policy nodes are sink-only runtime contracts and are invalid when
  configured with outgoing routes.
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

### Snapshot path posture
- Snapshot posture is canonical-only: `.vizier/narrative/snapshot.md`.
- Legacy `.vizier/.snapshot` discovery/path compatibility is removed.

### Plan doc durability vs merge-time plan removal
- Plan docs remain durable workflow artifacts in retained plan-visibility surfaces.
- Historical merge flows may have removed `.vizier/implementation-plans/<slug>.md` from target history after embedding plan context.
- Reconciliation: workflow durability does not imply target-branch permanence, especially across legacy command-era commits.

### Plan-doc/branch identity vs current inventory drift
- Intended contract: one plan slug maps to one `draft/<slug>` identity.
- Current repositories may show drift between plan docs and local draft branches; pending-plan surfaces can become less trustworthy in that state.
- Reconciliation: treat 1:1 as the target contract and keep transitional operator guidance until hygiene enforcement lands.

## Source Anchors

- Plan schema/front matter: `vizier-cli/src/plan.rs`
- Scheduler job records/metadata: `vizier-core/src/jobs/mod.rs`, `docs/dev/scheduler-dag.md`
- Workflow capability taxonomy + validator: `vizier-kernel/src/workflow_template.rs`
- Session schema/path: `vizier-core/src/auditor.rs`
- Clean-worktree exclusions: `vizier-core/src/vcs/status.rs`
