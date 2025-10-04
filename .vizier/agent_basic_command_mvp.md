Update (2025-10-04): Lock MVP on Workflow B (apply-to-branch) and Outcome facts.

Acceptance (product-level):
- Command: `vizier agent run [<todo-name>]` selects a TODO; non-interactive requires explicit name.
- Isolation: Create an isolated branch from current baseline; do not alter main or pre-existing staged changes.
- Outcome fields: { todo, branch, commit_count, pr_url? or review_instructions, gate_state } appear in outcome.v1.
- PR optional: Absence of remote or `--no-pr` downgrades to local review; still success with clear next steps (git commands) in Outcome.
- Save path: After merge, `vizier save` updates TODO and Snapshot with links to PR/merge and appends a Decision Log entry (AGENTS.md v1).
- Session logging: capture workflow_type=agent_basic, timestamps, branch, pr number/url if any.

Cross-links: Auditor facts, VCS orchestration, Outcome summaries, AGENTS.md schema, Session logging.

---

