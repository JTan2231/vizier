---
plan: more-tokens
branch: draft/more-tokens
status: draft
created_at: 2025-11-17T18:00:38Z
spec_source: inline
---

## Operator Spec
we need to get as close to this granularity as possible regarding token usage reporting: "Token usage: total=46,177 input=40,322 (+ 261,760 cached) output=5,855 (reasoning 4,471)". Reference ~/rust/codex for the source code of codex

## Implementation Plan
## Overview
Vizier already emits `[usage] token-usage — prompt=… (+Δ) completion=… (+Δ)` lines and CLI epilogues, but both only show prompt/completion totals (`vizier-cli/src/actions.rs:121`). The operator needs Codex-level granularity—total tokens, non‑cached input plus cached input breakdown, output totals, and reasoning output—for compliance and billing. Codex already exposes these fields via `turn.completed` events (`vizier-core/src/codex.rs:836`) and formats them in `Token usage: total=… input=… (+ … cached) output=… (reasoning …)` (`~/rust/codex/codex-rs/protocol/src/protocol.rs:869`). This plan captures that full breakdown across Codex-backed commands while keeping the existing delta reporting, so auditors and session logs carry the richer data even when the backend omits it.

## Execution Plan
1. **Capture full token usage from Codex events**
   - Extend `vizier-core/src/codex.rs:836` so `extract_usage` reads `cached_input_tokens`, `reasoning_output_tokens`, and `total_tokens` from `turn.completed` payloads (falling back to zero if missing) in addition to the existing `input_tokens`/`output_tokens`.
   - Update `CodexResponse` to carry the richer struct and keep propagating it through `run_exec` even in Passthrough mode when the data is available; keep `mock_codex_response` (`vizier-core/src/codex.rs:865`) in sync so integration tests exercising `VIZIER_SUPPRESS_TOKEN_USAGE` still work.
   - Reference Codex’s formatter (`codex-rs/protocol/src/protocol.rs:869`) while implementing the string builder so that Vizier’s messages match Codex semantics (non-cached input contributes to total; cached input/output reasoning stay in parentheses).
   - Acceptance: Inspect logging with a Codex-backed `vizier ask` run and confirm that `CodexResponse.usage` now contains all five fields whenever Codex emits them.

2. **Broaden the auditor’s data model**
   - Expand `auditor::TokenUsage` (`vizier-core/src/auditor.rs:31`) and `TokenUsageReport` (`vizier-core/src/auditor.rs:37`) so they track the extra totals plus deltas (e.g., cached input delta, reasoning delta, blended total delta).
   - Update `Auditor::add_message` (`vizier-core/src/auditor.rs:252`) to accept an optional `FullTokenUsage` payload supplied by Codex runs. Store that payload on the auditor state so `capture_usage_report_locked` (`vizier-core/src/auditor.rs:287`) can merge it into the next report while keeping backward compatibility for wire‑only runs (new fields stay `None` if the backend never provided them).
   - Ensure `TokenUsageReport::to_progress_event` and the `[usage] token-usage` summary honor quiet/verbosity gating while appending the new clauses only when populated.
   - Maintain deltas and totals derived from `wire::types::Message` input/output counts so existing workflows/tests depending on prompt/completion numbers keep working.

3. **Surface the richer breakdown everywhere operators see it**
   - Update CLI helpers in `vizier-cli/src/actions.rs` (`print_token_usage`, `token_usage_suffix`) so the final summary reads `Token usage: total=T (+Δ) input=I (+Δ, +C cached) output=O (+Δ, reasoning R)` when data exists, and falls back to today’s format otherwise. Include the agent annotation suffix as before.
   - Teach the session artifact writer to persist the new fields: extend `SessionTokenUsage` (`vizier-core/src/auditor.rs:1052`) so `.vizier/sessions/<id>/session.json` carries cached/ reasoning totals and deltas alongside prompt/completion counts. Update `SessionOutcome` serialization so downstream tooling can trust the richer schema.
   - Propagate the new fields into any downstream consumers (Outcome JSON once outcome.v1 lands, but today that means the session log plus human epilogue). Verify that `display::render_progress_event` output for `[usage] token-usage` remains one line and stays ANSI-free in non‑TTY contexts.

4. **Adjust tests/docs and add coverage**
   - Refresh integration helpers in `tests/src/lib.rs` (`parse_usage_line`, `UsageSnapshot`, `parse_session_usage`) to parse the extended format and assert that CLI stderr and session logs still match (`test_session_log_captures_token_usage_totals` at line 588). Add expectations for the cached/ reasoning numbers when Codex provides them and confirm they stay absent (zero) when usage is suppressed.
   - Extend `test_ask_reports_token_usage_progress` to ensure the `[usage]` progress line includes the optional clauses only when present and remains suppressed with `-q`.
   - Update developer-facing docs/README snippets if they describe the token usage format, ensuring they match the new string (scan `README.md` and `docs/workflows/draft-approve-merge.md` for “Token usage” references; currently none, so no change unless we add a short mention under troubleshooting).
   - Acceptance: `cargo test -p tests test_ask_reports_token_usage_progress`, `test_session_log_captures_token_usage_totals`, and `test_session_log_handles_unknown_token_usage` all pass; manual run of `vizier ask` with Codex shows the target string.

## Risks & Unknowns
- **Backend gaps**: The wire backend (and Codex passthrough mode) may never emit cached/ reasoning counts. The plan assumes optional fields and keeps today’s behavior when data is missing, but we should confirm we don’t regress usage totals in those scenarios.
- **Schema churn**: Session logs gain new fields; downstream tooling that parses `token_usage` must tolerate the richer object. Coordinate with any consumers before landing.
- **Delta math**: Codex reports per-turn totals, while Vizier currently tracks cumulative prompt/completion counts. We must ensure we don’t double-count cached tokens when building deltas and that total = non-cached input + output still holds.
- **Integration overhead**: If wire’s `wire::types::Message` ever needs to store the new numbers, we may have to update that upstream crate; for now we plan to stash extras on the auditor directly.

## Testing & Verification
- `cargo test -p tests test_ask_reports_token_usage_progress` to confirm stderr progress lines show/hide the enriched clause appropriately.
- `cargo test -p tests test_session_log_captures_token_usage_totals` and `test_session_log_handles_unknown_token_usage` to ensure session JSON mirrors the CLI and the unknown path still works.
- Manual Codex-backed `vizier ask "smoke"` run to inspect the `[usage] token-usage` line and final `Token usage: …` string for the new fields.
- Optional: run a wire-backed `vizier ask --backend wire …` (if supported) or a Codex run with `VIZIER_SUPPRESS_TOKEN_USAGE=1` to verify graceful degradation.

## Notes
- Narrative summary: drafted a plan to pipe Codex’s full token-usage breakdown (non-cached input, cached input, output, reasoning) through Vizier’s auditor so CLI epilogues, progress events, and session logs match Codex granularity without regressing the existing delta reporting.
