---
plan: better-scripting
branch: draft/better-scripting
---

## Operator Spec

Today: bundled shim pipes codex exec --json - | tee >(jq … >&2) | jq …, so stderr progress is block-buffered and arrives at the end.
Goal: move the fixed pipeline into Rust: build a shell wrapper around the agent command that tees JSON to a progress filter and extracts the final message from stdout.
User input shrinks to “progress filter command” only; the wrapper enforces stdout = final assistant text, stderr = progress, and handles buffering.
Operator still provides the underlying agent exec command (e.g., `codex exec --json`); config resolution must carry both the exec and the progress-filter command.
Buffering mitigations (pty or stdbuf) live in the wrapper, not in user scripts.

## Implementation Plan
## Overview
- Build a first-class agent output wrapper so Codex/Gemini JSON streams are split into live progress (stderr) and final assistant text (stdout) without user-managed tee/jq pipelines. This fixes block-buffered progress from the current bundled shim, tightens the stdout/stderr contract thread, and simplifies configuration to a single “progress filter” command.
- Impacted users: anyone running agent-backed commands (`draft/approve/review/merge/ask/save`) with the bundled Codex shim or custom scripts; they should see timely progress history and a consistent output contract with fewer config knobs.

## Execution Plan
1) **Define wrapper contract and config surface**
- Specify the new wrapper behavior: agent command emits JSON to stdout; wrapper tees it to a configurable “progress filter” command (stderr) and extracts the final assistant message for stdout, with buffering mitigations handled internally.
- Extend agent runtime config (`vizier-core/src/config.rs`) with explicit fields for the operator-provided agent exec command and an optional progress-filter command (with sensible defaults for Codex), preserving backward compatibility for existing “already filtered” scripts. Update `example-config.toml` and defaults to show the pairing and when the wrapper is enabled/disabled.

2) **Implement wrapper orchestration in the agent runner**
- Add a wrapper path in `vizier-core/src/agent.rs` (or a helper module) that, when configured, spawns the base agent command, streams its stdout into both the progress filter process (feeding ProgressHook) and a final-message extractor, and enforces stderr/stdout separation.
- Handle buffering by wrapping the pipeline with `stdbuf`/PTY or equivalent where needed, propagate non-zero exits from any leg with clear stderr context, and keep prompt writing unchanged. Ensure the agent response carries the extracted assistant text and collected stderr lines.

3) **Update runtime resolution and bundled shims**
- Adjust runtime resolution to build the wrapped command vector and annotate `AgentRequest` metadata so session logs record whether the wrapper was used and which filter ran.
- Simplify `examples/agents/codex.sh` to emit the JSON stream only (plus optional prompt preview), relying on the wrapper for progress/final extraction; ensure Gemini shim remains compatible or gains a no-op wrapper path.

4) **Thread through CLI display and logging**
- Verify `vizier-cli/src/actions.rs` uses the new metadata and ProgressHook so progress lines stream under quiet/no-ansi/TTY rules, and session logs capture the wrapper/filter choice for auditing.
- Ensure failure cases (invalid filter command, malformed JSON) surface as deterministic, non-ANSI stderr errors with appropriate exit codes.

5) **Testing & verification harness**
- Add unit tests in `vizier-core/src/agent.rs` for wrapped execution: a fake agent emitting JSON events with delays should yield timely progress events and the correct final assistant text; assert blocking is gone.
- Add a config-layer test to prove progress-filter config resolves via defaults/overrides and preserves legacy behavior when disabled. Consider an integration-style test that runs the wrapped codex shim with a trivial jq filter to assert stream ordering.

6) **Docs and operator guidance**
- Update `README.md`, `AGENTS.md`, and `docs/prompt-config-matrix.md`/workflow docs to describe the new wrapper contract, the progress-filter knob, defaults, and migration guidance for custom agent scripts. Call out the stdout/stderr contract and buffering guarantees to align with the active IO thread.

## Risks & Unknowns
- Existing custom agent commands that already emit final text could be double-wrapped; need a clear opt-out/back-compat flag.
- Buffering fixes may differ across platforms; PTY vs `stdbuf` choice could affect portability.
- Progress filter commands may have their own buffering/format quirks; need guardrails when filter output is empty or malformed.

## Testing & Verification
- Unit: wrapped runner streams progress promptly and returns the final message even with chunked JSON and sleeps between events.
- Unit/config: progress-filter config resolves with defaults and can be disabled to preserve legacy behavior.
- Integration/smoke: run an agent-backed command with the new default codex shim and confirm progress lines appear before completion, stdout only contains the final assistant text, and quiet/no-ansi behavior is unchanged.
- Error paths: invalid/missing filter command produces a clear stderr error and non-zero exit, without leaking partial stdout.
