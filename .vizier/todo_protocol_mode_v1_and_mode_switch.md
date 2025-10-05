Context
- Users report IO friction in mixed human/machine contexts. Codex pattern: a dedicated protocol mode for machine-machine operation, separate from conversational (chat) mode.
- Current snapshot already targets stdout/stderr contract, renderer-neutral events, and JSON streams, but lacks an explicit “mode” split.

Product intent
- Introduce two explicit run modes:
  1) Chat Mode (default): human-first CLI experience with TTY-gated progress, line-oriented text, optional ANSI, and readable Outcome epilogue. Accepts interactive input; no assumptions about stdin piping.
  2) Protocol Mode v1: machine-first contract. No interactive prompts. Input/Output are structured and stable; IO is safe for piping and orchestration.

Protocol Mode v1 requirements
- Inputs:
  - Accept commands via CLI flags and/or NDJSON on stdin (eventually). For v1, flags are sufficient; stdin reserved for future payloads; MUST NOT block if stdin is closed.
- Outputs:
  - stdout: Only structured JSON/NDJSON per outcome.v1 and event-stream.v1. No human chatter, no ANSI.
  - stderr: Only errors/warnings; gated by -v/-vv; no spinners.
  - Exit codes: 0 success, >0 categorized failures (usage, VCS, network, auth, internal). Acceptance test enumerates categories.
- Behavior:
  - Non-interactive by default; any operation requiring confirmation must fail fast with a clear Outcome.status=needs-confirmation unless explicit allow flags provided.
  - Deterministic ordering of events. Event schema versioned; include mode:"protocol" in headers/metadata.

Chat Mode requirements (alignment)
- Remains default. Keeps TTY-gated progress, human epilogue mirroring Outcome facts, and optional `--json`/`--json-stream` for users who want structure without switching modes.

Acceptance criteria
- `vizier <cmd> --mode protocol` produces only JSON on stdout and no ANSI escapes in both TTY and non-TTY. Stderr contains nothing on success at -q/-v, and minimal diagnostics at -vv.
- Outcome JSON includes {mode, command, success, exit_code, summary, facts:{...}} and matches Auditor/VCS facts.
- Same command in Chat Mode prints a human epilogue and (optionally) an Outcome JSON line when `--json` is passed.
- Integration tests cover piping, closed-stdin, and CI logs (non-TTY) for both modes.

Pointers
- vizier-cli/src/main.rs (flag parsing), vizier-core::display (progress/ANSI), vizier-core::observer/auditor (Outcome facts), event-stream wiring once available.

Cross-links
- Thread: Stdout/stderr contract + verbosity (ACTIVE)
- Thread: Terminal-minimal TUI + renderer-neutral events (ACTIVE)
- Thread: Outcome summaries (ACTIVE)
