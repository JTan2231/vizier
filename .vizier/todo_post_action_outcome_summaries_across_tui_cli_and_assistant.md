Thread: Outcome summaries (CLI-first) — Cross-links: Stdout/stderr contract + verbosity; Mode split (Chat vs Protocol); Auditor; Session logging; Agent Basic Command

Tension
- After operations (especially chat-driven changes), users get inconsistent, verbose, or missing summaries. There’s no single authoritative epilogue line or JSON payload consumers can rely on across surfaces.

Desired behavior (product level)
- Emit a unified Outcome after every operation (chat, save, agent, init, config changes):
  • Human epilogue (Chat mode): compact, 1–5 lines, summarizing what changed using audited facts (A/M/D/R counts), gate state, and any workflow metadata (e.g., {todo, branch, commit_count, pr_url}).
  • Machine epilogue (JSON): outcome.v1 object printed to stdout when `--json` or `--json-stream`, and always in Protocol mode. Includes fields: operation, success, a/m/d/r, gate_state, mode, messages[], and optional workflow metadata.
- Deterministic placement: Outcome is the final emission for an operation. In stream mode, it is the final event of type "outcome".
- Respect IO contract: human epilogue to stdout in Chat mode; progress/status to stderr gated by TTY/verbosity; no ANSI in non-TTY and never in Protocol mode.
- Assistant final reply mirrors the human epilogue in Chat surfaces.

Acceptance criteria
1) After a chat that edits files, the CLI prints a one-line summary to stdout: "Outcome: A=#, M=#, D=#, R=#; gate=<state>" and, if applicable, "todo=<id> branch=<name> commits=# pr=<url>".
2) With `--json`, stdout contains only a single outcome.v1 JSON object for the operation. With `--json-stream`, NDJSON includes an "outcome" event last.
3) In `--mode protocol`, stdout contains only JSON/NDJSON; no human prose; exit codes categorized; ordering deterministic.
4) Outcome data sources are auditor-backed: facts must align with actual VCS/Auditor state. Tests assert consistency.
5) Assistant channel (chat) mirrors the human epilogue content succinctly.
6) Non-editing operations (e.g., no-op chat) still print an Outcome with success=true and a/m/d/r all zero.

Pointers (anchors)
- Core: vizier-core/src/auditor.rs (facts), display.rs (rendering), observer.rs (event stream), chat.rs (hooks)
- CLI: vizier-cli/src/actions.rs (flag plumbing, mode handling), main.rs
- Tests: tests/src/main.rs (stdout/stderr capture; NDJSON schema)

Notes
- Keep JSON schema small and versioned; prefer snake_case keys; include mode and verbosity for traceability.
- Defer renderer choices; Outcome plugs into the renderer-neutral event stream.
- Ensure closed-stdin never blocks; print Outcome even on handled failures with success=false.