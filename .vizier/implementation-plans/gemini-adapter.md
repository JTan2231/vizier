---
plan: gemini-adapter
branch: draft/gemini-adapter
---

## Operator Spec
we need a gemini display consumer, similar to the one we have to codex. see GEMINI.md. note that it was written from the perspective of the source code, ~/ts/gemini-cli

## Implementation Plan
## Overview
- Add a Gemini display consumer so Vizier can render Gemini backend progress/history in the same structured way as Codex, keeping stdout/stderr contracts intact and advancing the pluggable-agent posture.
- Impact: operators using a Gemini CLI/backend will see readable `[gemini] phase — message` lines, consistent token/usage handling, and preserved Outcome/session logging; Codex and wire behavior must stay unchanged.
- Need now: unblock Gemini backend adoption without regressing verbosity/ANSI rules or the agent backend abstraction.

## Execution Plan
1) **Event schema grounding**  
   - Read GEMINI.md (referenced in the spec) to extract the Gemini CLI event schema (fields for phase/label/message/detail/path/status/progress/timestamps, token usage shape, error patterns).  
   - Produce a mapping plan from Gemini payload fields to `ProgressEvent` slots, noting fallbacks and any deviations from Codex. Capture gaps/assumptions if GEMINI.md is unavailable.

2) **Backend/config surface**  
   - Decide how Gemini is selected: introduce a `gemini` backend variant (CLI `--backend gemini`, `[agents.*] backend = "gemini"`) or a config flag that routes to the Gemini display adapter while reusing the existing runner.
     - Operator note: preferably the former, unless we have strong reasons for the latter
   - Update backend resolution (`BackendKind::from_str`/CLI value enums/config parsing) and session logging so Gemini selection is recorded consistently without disturbing agent/wire defaults.

3) **Gemini display adapter implementation**  
   - Add a `GeminiDisplayAdapter` (new module alongside `codex.rs` or within `agent.rs`) that converts Gemini events into `ProgressEvent` values with `[gemini]` source, sane phase/message/detail/path/progress/status/timestamp extraction, and preserves raw payloads.
     - Operator note: preferably the former, unless we have strong reasons for the latter

   - Handle malformed/unknown events gracefully (fallback to raw payload text) and ensure no ANSI or TTY-specific behavior leaks from the adapter itself.

4) **Pipeline integration**  
   - Wire the Gemini adapter into agent execution: selection in `resolve_display_adapter`, propagation through `AgentRequest`/`AgentRunner` execution, and rendering via existing display hooks so progress honors verbosity/progress flags and non-TTY constraints.  
   - Confirm Codex/wire paths are unaffected and that Gemini runs still capture assistant text and usage metadata where available.

5) **Tests & fixtures**  
   - Add unit tests for the Gemini adapter covering happy-path field extraction, fallback behavior when fields are missing, and raw preservation (mirroring Codex tests).  
   - Add a lightweight integration-style test that feeds representative Gemini JSON events through the adapter/ProgressHook to assert emitted `[gemini] phase — message` lines and token-usage handling.

6) **Docs & operator cues**  
   - Update operator-facing docs (README, example-config) to list Gemini as a supported backend, show config/CLI selection, and note any capability/flag differences or limitations.  
   - Call out current status/limitations (e.g., no auto-remediation, usage data availability) so operators know what to expect.

## Risks & Unknowns
- Introducing a new backend variant may ripple into CLI/help/config precedence; need to ensure backward compatibility and avoid misclassifying Codex runs.
- Actual Gemini CLI invocation semantics (flags, stdin/stdout behavior) are unclear; the plan assumes runner compatibility or minimal adjustments.
- Token-usage reporting may differ; need a fallback when Gemini omits usage data without breaking Outcome/session logging expectations.

## Testing & Verification
- Unit tests for `GeminiDisplayAdapter` covering field extraction, fallback to raw payload, and source/phase formatting.
- (If feasible) adapter/ProgressHook smoke test emitting a sample Gemini event stream to assert rendered lines respect verbosity/progress settings (no ANSI in non-TTY).
- Regression checks to ensure Codex/wire adapters still produce existing outputs and CLI backend selection rejects unsupported combinations as before.
- Doc/help verification: `vizier --help` and config examples show the new backend option without conflicting with existing aliases.
