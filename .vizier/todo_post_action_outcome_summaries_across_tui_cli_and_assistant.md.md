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


---
Update (2025-10-02): Outcome summaries are now the canonical epilogue for actions initiated under DAP. Scope narrowed to CLI-first given lack of vizier-tui in this repo. Acceptance: After any assistant-initiated change (snapshot/TODO), the CLI prints a compact factual summary sourced from Auditor/VCS facts. Assistant final turn mirrors it. Tests to assert message presence and contents.


---


---
Renderer-neutral + terminal-minimal constraints (2025-10-02)
- Outcome delivery must work within the minimal-invasive terminal philosophy: no alt-screen/full redraws; line-oriented; status line collapses to a single Outcome line.
- Source outcomes from the renderer-neutral event stream once available (status/outcome events). CLI consumes and renders; `--json`/`--json-stream` expose machine-readable forms.
- Respect TTY gating: no ANSI control sequences in non-TTY contexts; human epilogue hidden with `--quiet`.
- Cross-link: See TODO “minimal_invasive_tui_and_renderer_neutral_surface” for the event contract and constraints.


---

Update (2025-10-04): Outcome CLI-first aligned with stdout/stderr contract and verbosity levers. The canonical epilogue is sourced from Auditor/VCS facts and emitted to stdout as outcome.v1 (when --json) or a compact human block otherwise. Assistant final mirrors the same facts. Add integration tests covering: (TTY x non-TTY) x (quiet, default, -v/-vv) ensuring no ANSI in non-TTY and stable Outcome presence. Cross-link tightened with stdout/stderr contract TODO and Agent Basic Command Outcome fields.

---


---
Status update:
- Auditor now backs the chat path, so Outcome summaries can source A/M/D/R facts reliably after chat operations.

Clarifications:
- Ensure the Outcome epilogue appears after every chat action and matches Auditor facts exactly.
- In protocol mode, the outcome.v1 JSON must include audited counts, file lists (optional, bounded), and gate state.

Acceptance criteria additions:
- For a chat that produces no changes, Outcome explicitly reports zero-diff state with a clear message and JSON {diff:false}.
- For destructive diffs with confirm_destructive=true, Outcome reflects "blocked: confirmation required" and no changes applied.


---

Canonicalization note (2025-10-06): This is the canonical TODO for Outcome summaries across surfaces. Duplicate consolidated: post_action_outcome_summaries_across_tui_cli_and_assistant.md.md (now a redirect stub).

---

