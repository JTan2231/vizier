Thread: Mode split — Protocol Mode v1

Tension
- Mixed human/machine outputs make automation brittle. We need a deterministic, machine-first protocol mode while preserving a human-friendly default.

Behavioral definition
- CLI flag: --mode protocol (default: chat)
- Protocol mode guarantees:
  1) Stdout carries only structured JSON/NDJSON events; no human prose; no ANSI anywhere.
  2) Event stream: renderer-neutral lifecycle events — {schema:"event.v1", type:"status|message|outcome", ts_ms, level?, message?, data?}. Ends with outcome.v1 object.
  3) Deterministic ordering: status events in causal order; no interleaving human text.
  4) Non-interactive by default; never opens an editor; respects --quiet for suppressing status events (still emits final outcome).
  5) Exit codes categorized: 0 success; 10 no_changes; 20 blocked_by_gate; 30 invalid_input; 40 vcs_error; 50 network_error; 70 internal_error.

Acceptance criteria
- --mode protocol flips IO behavior across all commands (ask/chat/save/agent run) per above.
- Non-TTY and TTY behave identically in protocol mode: no ANSI; stderr optional status per verbosity.
- NDJSON events validate against event.v1 and outcome.v1.
- Closed stdin never blocks; commands complete deterministically.
- Integration tests cover: protocol mode for chat/save; exit codes mapping; event ordering; absence of ANSI; quiet mode behavior.

Pointer-level anchors
- vizier-cli/src/main.rs, actions.rs: mode flag plumbing; exit code mapping.
- vizier-core/src/observer.rs and display.rs: event emission and stream discipline.
- tests/src/main.rs: NDJSON capture and schema validation; deterministic ordering assertions.

Cross-links
- Stdout/stderr contract: protocol relies on those IO rules.
- Outcome summaries: outcome.v1 object terminates the stream; same schema.
- Renderer-neutral events: this TODO grounds event.v1 and its fields.

Notes
- Keep event.v1 minimal to start; allow extension via data field with type-specific payloads.
