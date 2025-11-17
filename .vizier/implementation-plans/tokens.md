---
plan: tokens
branch: draft/tokens
status: review-fixes-in-progress
created_at: 2025-11-17T01:53:13Z
spec_source: inline
implemented_at: 2025-11-17T02:13:08Z
reviewed_at: 2025-11-17T02:20:24Z
---

## Operator Spec
we're not making token usages clear anywhere near enough. we need to be showing it at every opportunity/indication that we've used more during processes.

## Implementation Plan
## Overview
Operators currently get a single token summary appended to a few CLI epilogues (`vizier-cli/src/actions.rs`), so it is easy to miss how much Codex activity has accrued during longer multi-phase flows (draft → approve → review, save+commit, etc.). The Auditor already tracks per-message token counts (`vizier-core/src/auditor.rs`), and the Codex runner surfaces usage metadata when the backend provides it (`vizier-core/src/codex.rs`), but we are not highlighting the deltas as they happen. This plan standardizes usage reporting so every LLM interaction emits an immediate, auditable update, outcome epilogues show accurate totals, and session logs capture the same facts for downstream tooling. The change primarily impacts CLI users and multi-agent operators who rely on Vizier for compliance reporting.

## Execution Plan
1. **Make token accounting event-driven inside the Auditor**
   - Extend `vizier-core/src/auditor.rs` to retain the last-reported token snapshot (prompt/completion totals plus timestamps) and to compute deltas whenever the underlying message list changes. Hook both the Codex (`vizier-core/src/codex.rs::CodexRunner`) and wire backends so the usage record always lands even when Codex falls back to wire.
   - Provide a lightweight `TokenUsageReport { total, delta, known }` API that callers (CLI + display) can subscribe to without recomputing totals or hand-rolling locks. This report should surface immediately after each `llm_request*` completes, before we return control to CLI flows.
   - Acceptance signal: running `vizier ask ...` twice should produce two sequential reports whose second delta reflects only the second request, and an integration log shows we gracefully emit “unknown” only when the backend omits usage data.

2. **Expose live usage updates through the display/progress layer**
   - Add a renderer-neutral event in `vizier-core/src/display.rs` (e.g., `ProgressKind::TokenUsage`) that prints a single line like `token-usage — prompt=123 (+45) completion=67 (+12)` whenever the Auditor reports fresh totals. Respect verbosity/quiet settings so these lines disappear under `-q` but remain visible otherwise, and ensure non-TTY mode receives plain text with no ANSI adornment.
   - For Codex runs, thread the usage event emitter into the existing progress channel (`spawn_plain_progress_logger` and `display::call_with_status`) so that deltas land between `[codex] ...` events. For synchronous CLI contexts (e.g., commit message prompts), emit via `display::info` so the operator still gets the same line.
   - Acceptance signal: `vizier approve plan` should interleave token usage lines with `[codex] phase — message` history, and rerunning with `-q` must suppress those extra lines entirely.

3. **Wire usage facts into CLI epilogues, outcome summaries, and session logs**
   - Update `vizier-cli/src/actions.rs` helpers (`token_usage_suffix`, `print_token_usage`, and the various println! epilogues) to consume the new `TokenUsageReport`, showing both the running total and the latest delta. Where we already append `(tokens: ...)`, switch to a format like `(tokens: prompt=123 [+45] completion=67 [+12] total=190)` so reviewers can tell whether a new Codex pass just occurred.
   - Extend the human Outcome strings (currently ad hoc per command) and the future outcome.v1 JSON scaffolding to include `{token_usage: {prompt, completion, total, delta}}`. Ensure the shared Outcome helper uses the same data so CLI output, `--json`, and soon protocol mode never drift.
   - Persist the token totals/deltas into `.vizier/sessions/<id>/session.json` so auditors can retroactively verify consumption per run. This likely means adding a `token_usage` section to `SessionOutcome` or recording a stream of `operations` entries annotated with the same deltas.
   - Acceptance signal: after `vizier review plan`, the console epilogue, the session JSON, and the Auditor’s cumulative state all agree on the totals, and running `jq '.outcome.token_usage' .vizier/sessions/<id>/session.json` shows the same numbers the CLI printed.

## Risks & Unknowns
- **Backend coverage**: Codex frequently omits usage data, so we must keep the UX from spamming “unknown” lines. The plan assumes we can detect “no update” events and skip emitting deltas when totals are unchanged.
- **Concurrency**: The Auditor is global; if we eventually support overlapping LLM calls, we need to ensure the usage tracker remains thread-safe and that progress events arrive in order.
- **Noise vs. signal**: Emitting a line after every assistant tool call could overwhelm logs during verbose flows. We may need to collapse extremely small deltas or respect verbosity levels aggressively to keep output readable.
- **Outcome schema coupling**: The outcome.v1 JSON work is still in flight; we must coordinate field names to avoid churn once that thread ships a canonical schema.

## Testing & Verification
- **Unit coverage**: Add tests in `vizier-core/src/auditor.rs` that simulate successive `llm_request` calls (with and without usage data) and assert the new tracker reports sane totals/deltas and suppresses duplicates when usage is unchanged.
- **Display/CLI integration**: Expand the CLI integration tests (e.g., the existing `tests/src/lib.rs` fixtures) to run `vizier ask` and `vizier approve` under both default and `-q` verbosity, asserting that token usage lines appear (or are suppressed) as expected and that the appended suffix reflects the latest delta.
- **Session log serialization**: Extend the session logging tests to confirm `.vizier/sessions/<id>/session.json` now carries token usage fields matching the CLI epilogue, including the `unknown` path.
- **Codex + wire parity**: Add a mocked wire-backend test case to ensure we still emit usage deltas when the Codex runner falls back to wire, preventing regressions when the fallback kicks in during `vizier approve` or `vizier save`.

## Notes
- Narrative change summary: elevate token usage visibility from a once-per-command afterthought to a first-class audit signal that appears during progress updates, in final outcomes, and inside session artifacts.
- Coordinate naming with the Outcome-summaries TODO so the new `{token_usage: ...}` matches the schema that thread expects once implemented.
- This plan updates only `.vizier/implementation-plans/tokens.md`; the rest of the repo stays untouched until the plan is approved.
