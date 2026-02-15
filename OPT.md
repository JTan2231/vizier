# Feature Spec: Dependency-Derived Optimistic Scheduling (`OPT`)

## Status

- Proposed implementation spec.
- Scope date: 2026-02-15.
- This spec defines how to restore optimistic queueing without requiring `--after`.

## Purpose

Allow operators to enqueue dependent workflows early (for example `draft` -> `approve` -> `merge`) and rely on declared dependencies, not manual `--after` wiring.

Generalize the same behavior to custom template workflows.

## Problem

Today, cross-run optimistic ordering only works when callers manually supply `--after`.

Artifact dependencies (`nodes.needs`) already model prerequisite data, but scheduler behavior blocks terminal when an artifact is missing and no producer is currently known. That prevents "queue now, producer arrives later" flows unless callers pre-wire explicit job predecessors.

## Decision

Vizier MUST support dependency-derived optimistic scheduling as a first-class policy:

1. Workflows can opt into optimistic missing-producer behavior for artifact dependencies.
2. Stage templates can chain through artifacts (not explicit `--after`) and still queue optimistically.
3. Custom templates can use the same pattern through declared `needs`/`produces` contracts.

Explicit `--after` remains supported and authoritative when provided.

## Scope

In scope:

- Scheduler dependency resolution policy for missing producers.
- Workflow template policy surface to enable optimistic dependency behavior.
- Stage-template artifact contracts for `draft`/`approve`/`merge`.
- Docs and tests for operator usage and custom-template patterns.

Out of scope:

- Removing `--after`.
- Changing lock/approval/pinned-head semantics.
- Replacing composed single-run flows (`imports` + `links`); those remain valid.

## Policy Contract

Add template-level dependency policy:

```toml
[policy.dependencies]
missing_producer = "block" # default; "wait" enables optimistic behavior
```

Rules:

1. Default is `block` for backward compatibility.
2. `wait` means missing artifacts with no current producers are treated as waiting, not terminal blocking.
3. Policy applies to artifact dependencies only (`nodes.needs` -> `schedule.dependencies`).
4. `after` dependency behavior is unchanged.

## Scheduler Contract

Dependency evaluation for each required artifact:

1. Artifact present: dependency satisfied.
2. Artifact missing + at least one active producer (`queued|waiting|running`): `waiting_on_deps`.
3. Artifact missing + at least one succeeded producer: `blocked_by_dependency` (`missing <artifact>`).
4. Artifact missing + producers exist but none succeeded and none active: `blocked_by_dependency` (`dependency failed for <artifact>`).
5. Artifact missing + no producers:
   - `missing_producer = "block"`: `blocked_by_dependency` (`missing <artifact>`).
   - `missing_producer = "wait"`: `waiting_on_deps` (`awaiting producer for <artifact>`).

This keeps strict-mode failure signaling unchanged while enabling optimistic-mode future-producer scheduling.

## Stage Template Contract

Stage templates SHOULD declare explicit cross-run artifact contracts so optimistic queueing works without `--after`.

### Draft

- `persist_plan` MUST declare `produces.succeeded` for:
  - `plan_branch:{slug,branch}`
  - `plan_doc:{slug,branch}`

### Approve

- Entry/root node MUST declare `needs` for draft outputs:
  - `plan_branch:{slug,branch}`
  - `plan_doc:{slug,branch}`
- Terminal success path MUST produce a custom approval token artifact:
  - `custom:stage_token:approve:<slug>`

### Merge

- Entry/root node MUST declare `needs` for:
  - `custom:stage_token:approve:<slug>`
  - (optional) `plan_branch:{slug,branch}` for explicit source-branch presence gating.

### Contracts

- Templates MUST declare artifact contracts used by custom artifacts (for example `stage_token@v1`).
- Placeholder expansion in `nodes.needs`/`nodes.produces` continues using existing queue-time interpolation.

## Custom Workflow Generalization

Custom workflows get optimistic scheduling by contract, not hardcoded stage names:

1. Upstream flow/node declares `produces` artifact(s).
2. Downstream flow/node declares matching `needs`.
3. Template policy sets `missing_producer = "wait"` when "producer may arrive later" behavior is desired.

Recommended pattern for cross-flow gates:

- Produce terminal tokens as custom artifacts (for example `custom:stage_token:<flow>:<id>`).
- Consume those tokens from downstream roots.

## CLI/UX Contract

- No new required CLI flag.
- `vizier run <flow>` continues to enqueue immediately.
- In optimistic policy mode, missing producers show as `waiting_on_deps` instead of terminal block.
- `vizier run ... --after` still works and overrides ambiguity explicitly.

## Compatibility

Backward compatibility is preserved by default policy:

- Existing workflows remain strict (`missing_producer = "block"`).
- Repos opt in per template by setting `missing_producer = "wait"`.

## Acceptance Criteria

1. `vizier run draft`, then immediate `vizier run approve`, then immediate `vizier run merge` (same slug/branch) works without `--after` when stage templates declare the contracts above.
2. In optimistic policy mode, a consumer queued before its producer remains `waiting_on_deps` (not terminal blocked) until producer appears or resolves failure conditions.
3. In strict policy mode, current behavior is unchanged.
4. Custom two-flow template pair using shared custom artifacts behaves the same as stage templates.
5. `--after` chaining remains functional and unchanged.

## Testing Requirements

Coverage MUST include:

1. Scheduler unit tests for `missing_producer = block|wait` with all producer-state permutations.
2. Integration tests for stage chain without `--after` (`draft` -> `approve` -> `merge`).
3. Integration test where consumer is queued first and later unblocks when producer is enqueued (optimistic mode).
4. Integration test that strict mode still terminal-blocks on missing producers.
5. Contract validation tests for stage-template artifact declarations and custom `stage_token` usage.

## References

- `vizier-kernel/src/scheduler/spec.rs`
- `vizier-core/src/jobs/mod.rs`
- `vizier-kernel/src/workflow_template.rs`
- `.vizier/workflows/draft.toml`
- `.vizier/workflows/approve.toml`
- `.vizier/workflows/merge.toml`
- `docs/user/workflows/stage-execution.md`
- `docs/user/config-reference.md`
