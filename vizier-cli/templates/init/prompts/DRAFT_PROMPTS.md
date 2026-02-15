<mainInstruction>
Given a fresh operator spec plus the current snapshot, produce a Markdown plan that reviewers can approve before any code lands.

Guardrails:
- The work happens inside a detached draft worktree on branch `draft/<slug>`; you are drafting the plan only, not editing other files.
- Treat `.vizier/narrative/snapshot.md` as the canonical truth. If a request contradicts it, note the tension and describe how to reconcile it.
- Reference relevant crates/modules/tests for orientation, but avoid prescribing code-level diffs unless safety or correctness demands it.
- Highlight sequencing, dependencies, and observable acceptance signals so humans know exactly what work will happen.
- The Markdown you emit is the plan. Do not include meta-steps about writing/storing the plan document.

Output format (Markdown):
1. `## Overview` — summarize the change, users impacted, and why the work is needed now.
2. `## Execution Plan` — ordered steps or subsections covering the end-to-end approach.
3. `## Risks & Unknowns` — consequential risks, open questions, or mitigations.
4. `## Testing & Verification` — behavioral tests, scenarios, or tooling that prove success.
5. `## Notes` (optional) — dependencies, follow-ups, or additional coordination hooks.

The operator spec and snapshot are embedded below. Use them as evidence; do not invent behavior that is not grounded in those sources.

Respond only with the Markdown plan content (no YAML front matter). Keep the tone calm, specific, and auditable.
</mainInstruction>

## Plan Metadata
- plan_slug: {{persist_plan.name_override}}
- branch: {{persist_plan.branch}}
- plan_file: .vizier/implementation-plans/{{persist_plan.name_override}}.md

## Snapshot
{{file:.vizier/narrative/snapshot.md}}

## Operator Spec
{{persist_plan.spec_text}}
