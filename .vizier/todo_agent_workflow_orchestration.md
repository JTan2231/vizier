# Coordinate multi-agent workflow checkpoints

## Goal
Provide a guided workflow that lets operators orchestrate multi-agent runs where Vizier mediates each checkpoint: high-level discussion, snapshot wording sign-off, architecture doc drafting, implementation, code sign-off, and final `vizier save`. Keep every hop auditable through the Auditor/outcome stack so intent cannot drift between agents. (thread: Agent workflow orchestration)

## Why
- Today these steps are ad-hoc, so different agents can reinterpret the desired direction and bypass gates meant for humans-in-the-loop.
- Architecture doc enforcement, Pending Commit gates, and snapshot maintenance already exist but are not connected into a single runbook, making it hard to prove that each phase received human approval.
- Vizier is expected to be the conductor whenever operators bring in external agents; we need a consistent story for how sign-offs happen and how the VCS state ties back to those approvals.
- `vizier draft` now spins `draft/<slug>` branches and commits `.vizier/implementation-plans/<slug>.md`, but nothing in the workflow records when that plan is required, who approved it, or how it’s merged back, so agents still improvise around the drafting stage.

## Workflow scope
- Stage 1: Discuss high-level goals with Vizier; capture the proposed snapshot delta and mark it “awaiting sign-off.”
- Stage 2: Provide an approval affordance so the user can accept/revise the snapshot wording before proceeding. Outcome should record the decision.
- Stage 3: Launch or request an architecture/implementation doc draft (today via `vizier draft`), capture the resulting `.vizier/implementation-plans/<slug>.md` + `draft/<slug>` metadata, and require that reference before implementation starts. Reuse the architecture-doc gate rules.
- Stage 4: Approve the plan before work begins (e.g., `vizier approve <slug>`), capturing reviewer sign-off, optional notes, and plan status before merging the draft branch back toward the primary branch.
- Stage 5: Implementation phase where Codex/agents apply code changes, all routed through the Auditor + Pending Commit gate, preserving pre-existing staged work.
- Stage 6: User sign-off on the code diff, with Outcome noting whether destructive changes were confirmed.
- Stage 7: `vizier save` ties everything together, citing the architecture doc/plan reference and preserving the session log.

## Acceptance criteria
- Operators can start a workflow session (CLI-first) that clearly enumerates the seven stages and shows current progress; stages can pause/resume without losing context.
- Snapshot and architecture-doc stages have explicit “approve/revise” affordances, and approvals become part of the Auditor facts and session log.
- Attempting to enter the implementation stage without an attached architecture doc path (or `vizier draft` plan slug) is blocked with a clear Outcome reason; once attached, the doc reference propagates into pending commits and final save metadata.
- Implementation stage ensures every agent-applied change stays behind the Pending Commit gate; users can inspect diffs, accept, or reject before continuing.
- Plan approval surfaces the `.vizier/implementation-plans/<slug>.md` metadata, requires an explicit reviewer confirmation, and records the decision through `vizier approve <slug>` so the draft branch merge + plan status updates appear in Auditor facts and Outcome summaries.
- Code sign-off requires an affirmative confirmation step; once accepted, Outcome includes the sign-off timestamp/user and pending commit status.
- Final `vizier save` emits an Outcome detailing the completed workflow, doc reference, files changed, and session log path so auditors can replay the run.
- Tests cover happy path, revise-and-resume flow, missing doc blocking behavior, rejection at code sign-off, and persistence/resume across CLI invocations; non-TTY/protocol variants must emit the same facts without ANSI.

## Pointers
- CLI workflow orchestration (vizier-cli/src/actions.rs)
- Auditor + session logging (vizier-core/src/auditor.rs, .vizier/sessions/)
- Architecture doc gate + Pending Commit gate threads
- DRAFT.md (draft workflow design), APPROVE.md (pending approval flow), `.vizier/implementation-plans/`

## Implementation notes
- Reuse existing gates/outcome machinery; add workflow state as metadata rather than inventing parallel tracking. Ensure each transition is idempotent so multi-agent runs remain recoverable after interruptions.
