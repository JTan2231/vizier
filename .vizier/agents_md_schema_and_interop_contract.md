Update (2025-10-04): Define AGENTS.md v1 scaffold and post-action hook.

Contract structure (repo root AGENTS.md):
- Contract: what agent operations are supported, safety gates, and the Outcome schema reference (outcome.v1).
- Interop Guide: how external agents/tools should invoke Vizier (CLI examples) and how to consume outcomes/events.
- Decision Log (append-only): terse entries per agent-driven operation with fields: { when, who/agent_id, workflow, todo/thread IDs, branch, commits, pr_url, outcome_summary_ref }.

Behavior:
- After any agent-driven operation completing successfully, append a Decision Log entry; include links to TODO and Snapshot thread.
- CLI affords `vizier agents show|append` and notes in Outcome epilogue whether AGENTS.md was updated.

Acceptance:
- Repo contains AGENTS.md after first agent operation or via `vizier agents init`.
- Entries are single-paragraph, chronological, with stable anchors for cross-referencing.

Cross-links: Outcome summaries, Session logging, Agent Basic Command.

---

