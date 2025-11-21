---
plan: removing-plan-metadata
branch: draft/removing-plan-metadata
status: draft
created_at: 2025-11-21T16:16:41Z
spec_source: inline
---

## Operator Spec
Plan files carry status, spec_source, and several timestamps (created_at, reviewed_at/review_addressed_at, implemented_at) that we only set/print; nothing in the workflow depends on them. They’re just bookkeeping: front‑matter labels
  plus list-preview output and a couple of tests. Target: strip those fields from new/generated plans, drop the mutators/prints/tests/docs that mention them, and only keep the durable identifiers (plan, branch, spec text + plan body).

## Implementation Plan
## Overview
- Remove non-essential plan front-matter fields (`status`, `spec_source`, `created_at`, `reviewed_at`/`review_addressed_at`, `implemented_at`) so plans only keep durable identifiers plus the spec/plan content.
- Simplify CLI surfaces (draft preview, list output, review/merge flows) that were only reading or mutating those fields, keeping plan workflows functional without metadata clutter.
- Update docs/tests to reflect the leaner plan format and ensure legacy plan files with old fields remain readable.

## Execution Plan
1) **Define the new plan document shape**
- Update plan rendering to emit only `plan` and `branch` in front matter before the Operator Spec / Implementation Plan sections.
- Adjust plan parsing to ignore extra/unknown keys so older plan files with legacy metadata still load, but internal structures stop carrying/depending on the removed fields.

2) **Simplify plan metadata model and surfaces**
- Shrink `PlanMetadata`/`PlanSlugEntry` to the remaining fields (slug, branch, spec summary/excerpt); drop status/spec_source/timestamps, along with display helpers (e.g., `created_at_display`).
- Revise plan previews and `vizier list` output to omit removed fields while still giving operators useful orientation (e.g., slug/branch/summary).

3) **Remove status/timestamp mutation paths**
- Delete the `set_plan_status` helper and replace its call sites in review/approve/merge flows with no-op or alternative logging so workflows continue without plan-front-matter updates.
- Ensure review continues to handle commit/--no-commit behavior gracefully when there are no plan file edits (e.g., critique streamed, session log recorded, but no empty commits).

4) **Docs and prompt-story alignment**
- Update draft→approve→review→merge docs (and any README/AGENTS mentions) to remove references to plan status/timestamps/spec_source, describing the lean plan format instead.

5) **Tests and fixtures**
- Refresh unit/integration tests and fixtures that assert on front-matter fields or status updates (e.g., draft plan contents, list output, review status transitions).
- Add/adjust coverage to confirm plans render/parse with only `plan`/`branch` and that workflows no longer rely on status fields.

## Risks & Unknowns
- Losing status markers may reduce at-a-glance signals (e.g., “review-ready”); need to ensure updated outputs still give enough context or document the change clearly.
- Review flow previously relied on plan-file edits to create narrative commits; must verify no empty-commit errors and that session logs remain the audit trail.
- Existing plan files with legacy metadata should still parse; confirming backward compatibility is essential.

## Testing & Verification
- `cargo test -p vizier-cli plan::` (unit tests around plan rendering/parsing/helpers).
- Targeted integration cases: `cargo test -p vizier-cli test_draft_creates_branch_and_plan test_approve_merges_plan test_review_streams_critique test_merge_removes_plan_document`.
- Spot-check `vizier list` output format via existing integration tests or a small fixture repo if coverage is missing.

## Notes
- No repository edits yet; this plan only scopes the upcoming cleanup.
