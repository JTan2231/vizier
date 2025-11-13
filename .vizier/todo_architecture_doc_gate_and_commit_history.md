# Enforce architecture-doc gate for every code change

Thread: Architecture doc gate + compliance

Tension
- Large-org governance now expects every code change to cite an architecture document, but Vizier neither scaffolds those docs nor blocks merges/saves when the doc is missing. Commit history offers no durable linkage back to design intent.

Desired behavior
- During planning and before `vizier save`, Vizier prompts for the architecture doc that justifies the change (existing file or new scaffold). Docs live under `docs/architecture/` with metadata (snapshot moment, authors, scope) so auditors can trace intent later.
- Outcome summaries (and any optional decision logs) must list the doc path/ID alongside the TODO or branch info, ensuring downstream tools know the associated design.
- Save/merge gates fail fast if a change lacks a referenced architecture doc or if the doc hasn’t been committed. Operators can choose to generate a scaffold on the spot, but they cannot bypass the requirement.
- Commit metadata (conversation commit and/or .vizier commit) records the doc path and a short summary so future readers understand which doc authorized the work.
- When multiple agents collaborate on one change, Vizier tracks a single shared architecture doc reference to avoid drift and keeps that reference consistent across branches/PRs.

Acceptance criteria
- `vizier save` refuses to finalize when no architecture doc is attached; Outcome shows `blocked: missing architecture doc`.
- Providing a doc path (existing or newly scaffolded) unblocks the gate; the Outcome cites the path, and the conversation/.vizier commits include the same reference.
- Docs created through this flow carry metadata tying them to the triggering TODO/thread and snapshot moment.
- Commit history (conversation or code commit message trailers) includes an `Arch-Doc:` line pointing to the doc, so external reviewers can verify compliance.
- Multi-agent runs share the same doc reference; attempts to land a branch with a different doc are rejected until reconciled.

Pointers
- docs/architecture/ (future scaffold)
- vizier-cli save flow
- Auditor + Outcome summaries (doc reference reporting)
