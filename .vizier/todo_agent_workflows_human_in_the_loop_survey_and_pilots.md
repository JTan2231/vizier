# Agent workflows: human-in-the-loop survey and pilots

Thread: Control levers surface + Commit isolation/gates + Outcome summaries (snapshot: Running Snapshot — updated)

Problem/tension
- We want to leverage code-focused agents (e.g., Codex/Claude Code analogs) inside Vizier, but the precise division of labor between agent and human is unclear. Current snapshot orients around commit gates and factual outcome summaries, but lacks explicit workflows that define who proposes vs. who approves vs. who executes changes.

Proposal (tentative; learning-oriented)
- Introduce two pilot workflows and instrument them to learn where the human should sit. Keep scope minimal and reversible.

Workflow A: Agent-Propose, Human-Stage, Vizier-Gate
- Behavior: Agent drafts a change set (no writes to repo). Human reviews proposed diffs in Chat TUI, stages selected hunks/files. Vizier enforces Pending Commit gate and renders Outcome summary.
- Acceptance:
  1) After an agent request, a "Proposed Changes" panel appears with file/hunk list and inline diffs; no filesystem writes until human stages.
  2) Human can stage per-hunk/per-file; unstaged proposals are discarded without side effects.
  3) Outcome summary shows counts for proposed vs. staged vs. committed; matches Auditor facts.
- Pointers: chat.rs (UI affordance), auditor.rs/vcs.rs (sourcing facts), editor.rs (avoid writes until staged).

Workflow B: Agent-Apply-to-Branch, Human-Approve, Vizier-Merge via Gate
- Behavior: Agent applies changes to a temporary, isolated branch (or working tree dir). Human approves in gate; Vizier merges via standard gate, preserving pre-existing staged changes untouched.
- Acceptance:
  1) A distinct workspace label (e.g., scratch/<id>) is visible in header/meta and Outcome summary.
  2) Approve action fast-forwards or merges the scratch change set through the same Pending Commit gate rules.
  3) Abort cleanly deletes scratch workspace and leaves working tree identical to pre-op state.
- Pointers: vcs.rs (branch/scratch orchestration), bootstrap.rs (workspace init), display.rs (header/meta).

Cross-cutting acceptance (both pilots)
- No auto-commits unless auto_commit=true; honor confirm_destructive.
- Assistant final message uses standardized Outcome summary blocks.
- Session logging records the workflow type, proposed/apply counts, and decision (approve/abort) in session.json.

Telemetry for learning (lightweight)
- Log timestamps and counts for: proposal size, time-to-stage, abort/approve, per-hunk acceptance ratio. Stored only in session.json, not external services.

Out of scope (for now)
- Tool/library mandates, persistent agent memory, concurrent agents, long-running background jobs.

Implementation Notes (allowed: safety/correctness)
- Ensure zero side effects before human staging in Workflow A.
- Ensure abort is idempotent and leaves no stray files/branches in Workflow B.
