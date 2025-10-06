
---
Update (Decision): Select Workflow B as MVP for Basic Agent Command

Rationale
- Aligns with user request: pick a TODO → apply changes in isolated branch → open PR/await review → finalize on merge with `vizier save`.
- Preserves safety: no changes land on main without explicit human action; abort is clean.

Scope for MVP
- Entry point: `vizier agent run [<todo-name>]` (CLI). If omitted and in TTY, interactive picker; non-interactive requires explicit name.
- Branch: Create isolated branch for the operation (e.g., agent/<id>-<slug>), apply agent changes there. Do not touch existing staged changes.
- PR (optional): If remote is configured and policy allows, open a PR targeting base branch; otherwise output next steps for local review without failing.
- Finalization: After PR merge, operator runs `vizier save` to record Outcome, link PR, and update TODO/snapshot status.

Acceptance (product level)
1) Running `vizier agent run <todo>` creates an isolated branch from the current baseline, applies agent-authored commits, and leaves main unchanged.
2) Outcome summary after the run includes: selected TODO, branch name, commit count, and PR URL if opened (or clear "awaiting review" state with local next steps).
3) If no remote exists or `--no-pr` is set, command succeeds and prints exact review instructions (git diff/checkout commands) in the Outcome.
4) `vizier save` after merge updates the TODO (e.g., mark Done or append resolution), links the PR/merge commit, updates the snapshot thread, and writes session log entries.
5) Honors existing gates: confirm_destructive/auto_commit, and preserves any pre-existing staged changes untouched.
6) Session logging captures: workflow_type=agent_basic, selected_todo, branch, pr_url/number (if any), decision (merge/abort), timestamps.

Pointers
- CLI wiring: vizier-cli/src/actions.rs
- VCS orchestration: vizier-core/src/vcs.rs
- Prompt composition: vizier-core/src/chat.rs (templated prompt from TODO + snapshot)
- Outcome/Auditor facts: vizier-core/src/auditor.rs


---

Deprecation (2025-10-06): Superseded by todo_agent_workflows_human_in_the_loop_survey_and_pilots.md. This file now serves as a redirect stub for backlinks.

---

