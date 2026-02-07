# Git hygiene and commit practices

Thread: Git hygiene + commit practices (cross: Commit isolation + gates, Architecture doc gate + compliance, Outcome summaries, Agent workflow orchestration)

Snapshot anchor
- Active thread — Git hygiene + commit practices (Running Snapshot — updated).

Tension
- Vizier’s Auditor and CommitMessageBuilder already stamp commits with session IDs and Codex summaries, but there is no clear, documented guidance on how commits should be structured (code vs `.vizier` vs narrative), how agent-authored commits should sit alongside human history, or how commit messages should consistently reference snapshot threads, plans, and architecture docs. This makes “good Git hygiene” hard to enforce and history harder to audit at a glance.

Desired behavior (Product-level)
- Provide a repository-level Git hygiene policy for Vizier-driven workflows that explains expected commit boundaries (what belongs in `.vizier`-only commits vs mixed code changes, how to use Pending Commit gates, and when to split changes) and how those boundaries relate to snapshot threads, narrative notes, and architecture docs.
- Ensure CommitMessageBuilder and CLI flows apply that policy consistently across assistant-backed commands (`vizier ask`, `vizier save`, `vizier draft`, `vizier approve`, `vizier review`, `vizier merge`) whenever they create commits, including how subjects are derived from Codex summaries, which trailers (Session ID, architecture doc reference, plan slug, gate status) should appear, and how human-supplied messages interact with generated ones.
- Make these practices visible and safe-by-default: a fresh repo using Vizier should get readable, review-ready commit history without configuration, while teams with stricter governance can tighten expectations through docs and light configuration rather than patching code.

Acceptance criteria
- A Git hygiene guide lives under `docs/` (or equivalent) and describes:
  - How Vizier expects commits to be structured across `.vizier` vs code vs narrative changes, including examples for common flows (ask/save/draft/approve/review/merge).
- Recommended subject/body conventions for agent-generated commits, including how they reference snapshot threads, narrative artifacts, implementation plans, and architecture docs.
  - How Pending Commit gates and `--no-commit` interact with these guidelines so operators understand when Vizier will create commits automatically vs hold changes for manual review.
- CommitMessageBuilder behavior and CLI commit flows are aligned with the guide, including:
  - Always producing a clear, descriptive subject line even when Codex summaries are sparse, while preserving human-authored subjects when provided.
  - Emitting a predictable set of metadata trailers (Session ID and, when available, architecture doc path, plan slug, and gate outcome) so downstream tooling can rely on them.
  - Ensuring .vizier-only commits, mixed commits, and human-only commits all retain the required metadata without duplicating or silently dropping fields.
- Outcome epilogues and session logs surface the same commit-hygiene facts (which commits were created, what metadata was attached, whether changes remain pending) so auditors and agents can reason about history without re-parsing Git directly.
- Tests cover representative flows (ask/save/draft/approve/review/merge) and assert that commits created by Vizier follow the documented patterns for both code and `.vizier` changes, including correct subjects and trailers.

Pointers
- `vizier-core/src/auditor.rs::CommitMessageBuilder`
- `vizier-cli/src/actions.rs` commit flows for ask/save/draft/approve/review/merge
- `.vizier/narrative/threads/architecture_doc_gate_and_commit_history.md` for architecture-doc linkage and commit history expectations
- Snapshot threads: Commit isolation + gates; Architecture doc gate + compliance; Outcome summaries; Agent workflow orchestration

Update (2025-11-21): Plan workflow steps already co-commit code edits with `.vizier/narrative/snapshot.md` and narrative updates while filtering `.vizier/implementation-plans/<slug>.md`, `.vizier/tmp/*`, and session logs out of staging/merge targets. Treat that behavior as the current baseline when shaping the commit-hygiene guidance.
Update (2025-11-21): Commit prompts default to the Linux kernel-style template (`type: imperative summary` ≤50 chars, wrapped 72-col body, `Signed-off-by` trailers) unless a repository override is provided via `.vizier/COMMIT_PROMPT.md` or `[prompts.commit]`; consider this the baseline when defining commit hygiene expectations.
Update (2026-02-06): Current repo state shows plan inventory drift (`.vizier/implementation-plans/refactor.md` + `removing-wire.md` while local draft branches are `draft/after` + `draft/retry`), so this thread now explicitly includes plan-doc/branch reconciliation as part of Git hygiene and auditability.
Update (2026-02-07): Refreshed the live inventory example: `.vizier/implementation-plans/` still holds `refactor.md` + `removing-wire.md` while local branches now include `draft/after`, `draft/init`, and `draft/patch`, confirming the drift is still active and widening.
