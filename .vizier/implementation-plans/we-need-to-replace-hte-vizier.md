---
plan: we-need-to-replace-hte-vizier
branch: draft/we-need-to-replace-hte-vizier
status: draft
created_at: 2025-11-17T06:07:25Z
spec_source: inline
---

## Operator Spec
we need to replace hte VIZIER CODE CHANGE or VIZIER NARRATIVE CHANGE commit message headers with something more descriptive. the existing feat/chore/etc.: headers for the code are good. the narrative headers could be similarly descriptive for what changed in the narrative

## Implementation Plan
- Added the requested planning doc describing how to replace the generic `VIZIER CODE/NARRATIVE CHANGE` headers with descriptive summaries, including context on why the change matters for auditors (`.vizier/implementation-plans/we-need-to-replace-hte-vizier.md:1`).
- Broke the implementation into concrete steps (contract definition, builder changes, call-site updates, docs, and tests) with specific code anchors to guide execution (`.vizier/implementation-plans/we-need-to-replace-hte-vizier.md:5-24`).
- Captured key risks plus the validation strategy so reviewers know what to watch and how to verify success (`.vizier/implementation-plans/we-need-to-replace-hte-vizier.md:25-37`).

Testing:
- Not run (plan-only work).

Outcome: Implementation plan drafted for improving commit headers.
