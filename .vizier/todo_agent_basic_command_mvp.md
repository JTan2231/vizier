Thread: Agent workflows (Workflow B) — see todo_agent_workflows_human_in_the_loop_survey_and_pilots.md; Snapshot: Active thread “Agent Basic Command (Workflow B MVP)”

Problem/tension
- Operators want a single, predictable flow to have an agent draft changes safely while keeping human review as the gate. Today there is no first-class command to drive this end-to-end.

Desired behavior (product level)
- Command: `vizier agent run [<todo-name>]`
  • If <todo-name> omitted and in TTY, present an interactive picker limited to existing TODOs; non-interactive requires explicit <todo-name>.
  • Compose a templated prompt that embeds: the selected TODO body and the current Snapshot (narrative + code state). Include short repo context hints without leaking secrets.
  • Create an isolated branch (agent/<id>-<slug>) from the current base branch; apply agent-authored commits there. Main remains untouched.
  • If remote is configured and policy allows, open a PR targeting the base branch; otherwise, mark state as “awaiting local review” and surface exact git commands for review.
  • On completion, print a single-line Outcome plus a short block detailing: selected TODO, branch name, commit count, PR URL/number or local review steps.
  • No destructive changes without confirm_destructive and auto_commit policies honored.

Finalize via save
- After PR merge (or manual fast-forward), operator runs `vizier save`.
  • Updates the selected TODO (append Resolution with PR/merge link; mark Done if appropriate).
  • Updates Snapshot threads to note the change landed (cite branch/PR).
  • Writes session log entries capturing workflow_type=agent_basic, selected_todo, branch, PR URL/number, decision (merge/abort), timestamps.

Acceptance criteria
1) Running `vizier agent run <todo>` results in a new branch with only agent-authored commits; main is unchanged; any pre-existing staged A/M/D/R remain untouched.
2) Outcome summarizes: TODO id/title, branch name, commit count, PR URL if opened; if no remote/--no-pr, Outcome includes exact local review commands.
3) Non-interactive runs require explicit <todo>; interactive picker appears only on a TTY.
4) `vizier save` after merge updates TODO and Snapshot and writes session JSON with the workflow metadata.
5) Abort path (`--abort` or failure) deletes scratch branch/worktree and leaves tree identical to pre-run state.
6) All outputs respect terminal-minimal constraints; JSON stream emits structured events for message/tool/status/outcome.

Pointers (anchors)
- CLI: vizier-cli/src/actions.rs and main.rs (new subcommand wiring; TTY detection; flags like --no-pr, --abort)
- Core/VCS: vizier-core/src/vcs.rs (branch/PR orchestration), auditor.rs (facts for Outcome), chat.rs (prompt composition), display.rs (Outcome rendering)
- Tests: tests/src/main.rs (integration harness; fake TTY; NDJSON schema validation)

Notes
- Library/tool choices for PR creation are open; tolerate no-remote and private forks gracefully.
- Security: redact secrets in prompts; cap snapshot length to budget.
- Performance: stream partial progress via status events; avoid long silent waits.
Update (2025-10-04): Product acceptance clarified and cross-linked:
- Command: `vizier agent run [<todo-name>]` with interactive picker only on TTY.
- Isolation: separate branch; main/staged changes untouched.
- Outcome: includes TODO id/title, branch name, commit count, PR URL if opened or exact local review steps; gate_state captured.
- Save: `vizier save` updates TODO and Snapshot; AGENTS.md Decision Log append.
- Session logging: capture workflow_type=agent_basic and branch/PR metadata.
- Respect stdout/stderr contract and terminal-minimal constraints. Anchors: vizier-cli/src/actions.rs, vizier-core/src/vcs.rs, auditor.rs, chat.rs, display.rs; tests in tests/src/main.rs.

---

