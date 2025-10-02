Thread: Outcome summaries across interfaces. Depends on Snapshot: Running Snapshot — updated (Outcome summaries: implement standardized component and prompt nudge).

Problem (behavioral):
- After actions (ask/chat/apply/save), users don’t get a concise, factual summary of what actually occurred. TUI and CLI messages are inconsistent; assistant final response is verbose and loosely coupled to audited changes.
- This causes uncertainty about: files touched, hunks staged, commits created, gates encountered, and what to do next.

Desired behavior:
- Standardize a compact “What happened” Outcome Summary available in both CLI (epilogue) and TUI (right/foot pane), sourced from Auditor/VCS facts. Assistant final message mirrors this structure.

Acceptance criteria:
1) Every user-visible action path (CLI ask, CLI save, Chat TUI apply/continue) ends with an Outcome Summary containing:
   - Operations: action label (e.g., ask/apply/save), elapsed time, model used.
   - Changes: counts for files A/M/D/R, hunks, and lines +/-.
   - Commits: whether a conversation commit occurred, whether a .vizier commit occurred, and whether a code commit occurred (Y/N) with SHAs if created.
   - Gates: whether Pending Commit gate is open, accepted, rejected, or skipped (and why: auto_commit, non_interactive, or no changes).
   - Next steps: 1–2 imperative suggestions (e.g., “Press A to accept pending commit” in TUI, or “Run `vizier save` to commit” in CLI).
2) TUI: Dedicated Outcome panel that auto-refreshes after tool calls and on gate transitions; never obscures the diff pane. Minimal key to toggle expand/collapse.
3) CLI: Epilogue block printed at the end of commands. Hidden with `--quiet`; JSON with `--json`.
4) Assistant final message: Ends with a terse Outcome block that exactly matches the Auditor facts. If no changes, clearly states “No code changes were created.”
5) Tests cover: presence/format under (a) no changes, (b) pending commit open, (c) auto-commit on, (d) rejected changes, (e) failure paths (shows error summary).

Pointers:
- Auditor facts source; VCS helpers for counts; vizier-cli/src/actions.rs for CLI epilogues; chat TUI render pane (vizier-core/src/chat.rs or TUI surface if present).

Implementation Notes (justified: safety/correctness):
- Outcome must reflect actual repo state post-action. Derive from VCS/Auditor, not model text. Ensure atomicity: compute after any writes and before process exit; in TUI, recompute on each state transition.
- For JSON output, use a stable schema to support scripts; version it as { schema: "outcome.v1", ... }.Update [2025-10-02]: Scope Outcome delivery CLI-first; defer TUI panel until a UI surface exists.
- Prioritize: CLI epilogue + Assistant final message alignment using Auditor/VCS facts. Provide `--json` machine format (schema: outcome.v1) and human epilogue by default; hide with `--quiet`.
- Defer: Dedicated TUI Outcome panel and live auto-refresh until a TUI surface/crate is present. Keep product requirements, but mark as blocked by UI surface availability.
- Tests: Add coverage for CLI epilogue presence/format under cases (a) no changes, (b) pending commit open, (c) auto-commit on, (d) rejected changes, (e) error path. JSON schema validation included.
- Cross-link: DAP thread must emit a one-line Outcome even when only .vizier changes occur. Integration tests should assert the line appears.


---

