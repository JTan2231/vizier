---
plan: list-prettify
branch: draft/list-prettify
status: draft
created_at: 2025-11-21T18:30:13Z
spec_source: inline
---

## Operator Spec
we recently merged a change--prettify--that made summary outputs nicer to look at. vizier list does not follow suit--it needs updated to match this new format

## Implementation Plan
## Overview
- Align `vizier list` output with the new prettified label/value block used across other CLI outcomes, improving readability of pending plan inventories.
- Impacted users: operators and agents inspecting draft branches via `vizier list` or the deprecated `approve --list`, plus any tooling that parses list output.
- Motivation: the recent prettify work standardized summaries elsewhere; list still emits legacy `plan=…` lines and now needs to match the shared format while honoring the stdout/stderr contract.

## Execution Plan
1) **Clarify target format and fields** — Inspect the existing prettified helpers (`format_block`, `format_label_value_block`, `format_number`) and the data returned by `PlanSlugInventory` (slug, branch, summary, and any status/reviewed_at metadata if present) to decide the per-entry rows (e.g., Plan/Branch/Summary/Status/Reviewed). Confirm how to present the “no pending plans” case in the new style and whether to include entry separators for multi-plan output.
2) **Refactor list rendering** — Update `list_pending_plans` to build rows per entry and render them through the shared block formatter (reusing the existing helpers in `actions.rs`). Preserve summary sanitization, keep output on stdout with no ANSI, and ensure the change also flows through the `approve --list` shim. Add spacing between entries (and/or a count header) so multiple plans remain scannable in the block layout.
3) **Update docs and help cues** — Review `docs/workflows/draft-approve-merge.md` (and any other mentions) to ensure wording matches the new presentation; add a short example if helpful. Keep behavior notes about the `--target` override intact.
4) **Adjust tests and add coverage** — Revise `tests/src/lib.rs::test_approve_merges_plan` to assert against the new block output (e.g., `Plan:`/`Branch:`/`Summary:`). Add/extend a targeted check that multiple entries render with the prettified layout and that the “no pending draft branches” path still surfaces clearly.

## Risks & Unknowns
- Output shape change may break scripts or agents parsing the old `plan=...` lines; need to decide if we supply a compatibility shim or release note.
- Handling multiple entries (spacing, alignment) could still feel crowded; may need tuning once seen in practice.
- If `PlanSlugInventory` doesn’t actually surface status/reviewed_at despite the snapshot claim, we’ll have to reconcile whether to extend the data model now or defer.

## Testing & Verification
- `cargo test -p tests test_approve_merges_plan` (and any new list-format assertions).
- Manual `vizier list` (default and with `--target`) to confirm prettified blocks for: multiple plans, single plan, and the empty case; ensure no ANSI and sensible spacing in non-TTY.
- Smoke `cargo test -p vizier-cli` if any unit helpers are added for formatting.

## Notes
- Narrative: drafted a plan to bring `vizier list` onto the shared prettified summary format; no code changes made yet.
