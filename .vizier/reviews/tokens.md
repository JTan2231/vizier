---
plan: tokens
branch: draft/tokens
target: master
reviewed_at: 2025-11-17T02:20:24Z
reviewer: codex
---

## Plan Alignment

- Plan document `.vizier/implementation-plans/tokens.md` declares `status: implemented` with `implemented_at`, and the code changes are tightly scoped to token-usage accounting, progress reporting, and session logging; no unrelated surfaces appear in the diff.
- Auditor changes match the “event-driven token accounting” step: `TokenUsageReport` plus `last_usage_report` and `latest_usage_report()` are added to track totals/deltas and avoid duplicate reports (`vizier-core/src/auditor.rs:35`, `vizier-core/src/auditor.rs:336`), and `add_message`/`replace_messages` now compute a report on assistant messages and Codex/wire responses rather than just overwriting the message list.
- Progress-layer exposure aligns with the plan: `ProgressKind::TokenUsage` with `[usage]`/`token-usage` labeling is introduced (`vizier-core/src/display.rs:113`), and `TokenUsageReport::to_progress_event` plus `forward_usage_report` emit either a progress event over the existing status channel or a plain stderr line that respects `Verbosity::Quiet` (`vizier-core/src/auditor.rs:45`, `vizier-core/src/auditor.rs:84`).
- CLI epilogues and session logs are wired as described: `print_token_usage` and `token_usage_suffix` now prefer `Auditor::latest_usage_report()` and show `(tokens: prompt=X [+Δ] completion=Y [+Δ] total=Z [+Δ])` when usage is known (`vizier-cli/src/actions.rs:121`), while the session JSON includes an optional `token_usage` block derived from the last report via `SessionTokenUsage` on `SessionOutcome` (`vizier-core/src/auditor.rs:1005`, `vizier-core/src/auditor.rs:1030`).
- The one notable gap versus the Execution Plan is testing: the existing `auditor` tests still only cover project-root detection (`vizier-core/src/auditor.rs:1048` onward), and there are no new unit or integration tests exercising token-usage deltas, unknown-usage behavior, or CLI suffix formatting, even though those were explicitly called out in the plan’s “Testing & Verification” section.

## Tests & Build

- `cargo check --all --all-targets` completed successfully with exit code 0; the logs show all workspace crates (including `vizier-core` and `vizier-cli`) building without errors, and there are no failing or skipped targets reported.
- `cargo test --all --all-targets` also succeeded with exit code 0; 9 integration tests in `tests`, 15 tests in `vizier-cli`, and 31 tests in `vizier-core` all passed with no failures or ignored cases, indicating the new token-usage wiring is at least type- and behavior-compatible with existing test coverage.
- No additional check commands were run beyond these two Cargo invocations, and there are no failing steps to call out; the current implementation has no automated tests that directly assert the new token-usage progress events or session-log fields.

## Snapshot & Thread Impacts

- The “Session logging to filesystem” active-thread bullet in `.vizier/.snapshot` now states that each outcome carries a `token_usage` totals+delta block for auditors (`.vizier/.snapshot:19`), which is consistent with the new `SessionOutcome.token_usage` field and `SessionTokenUsage` representation (`vizier-core/src/auditor.rs:1005`, `vizier-core/src/auditor.rs:1030`).
- The “Code state” section’s token-usage line has been tightened to describe event-driven reporting: `[usage] token-usage — prompt=X (+Δ) completion=Y (+Δ)` progress lines, CLI epilogues with totals+deltas, quiet-mode suppression, and a fallback to a single “unknown” line when backends omit usage (`.vizier/.snapshot:23-26); this matches the combination of `TokenUsageReport::to_progress_event`, `emit_token_usage_line`, and the updated CLI suffix (`vizier-core/src/auditor.rs:45`, `vizier-cli/src/actions.rs:121`).
- No TODO artifacts were added, removed, or explicitly closed in this branch; the work advances the existing “Session logging” and “Outcome summaries/stdout-stderr contract” threads by enriching the session outcome with token-usage data and emitting progress events, but leaves their broader acceptance criteria (schema validation, outcome.v1 JSON, mode-split behavior) still open as described in their current TODO files.
- There are no visible violations of snapshot promises: the new behavior is limited to usage-reporting and logging, and the narrative claims about event-driven token-usage lines and per-outcome `token_usage` blocks are backed by the introduced code for LLM-involving runs.

## Action Items

- Add unit tests around `TokenUsageReport` and `Auditor::capture_usage_report_locked` to verify cumulative totals, per-call deltas, duplicate-suppression, and the `usage_unknown`/`known=false` path (`vizier-core/src/auditor.rs:35`, `vizier-core/src/auditor.rs:320`).
- Extend the CLI integration tests to assert that `[usage] token-usage — …` progress lines appear (and are suppressed under `-q`) during a `vizier ask` or `vizier approve` flow, and that the final summary line includes the new `(tokens: prompt=X [+Δ] completion=Y [+Δ] total=Z [+Δ])` suffix (`vizier-cli/src/actions.rs:121`, `vizier-core/src/display.rs:113`).
- Add a serialization test for session logs that loads `.vizier/sessions/<id>/session.json` after a Codex-backed command and asserts that `outcome.token_usage` matches `Auditor::latest_usage_report()` values, including the `known=false` “unknown” path (`vizier-core/src/auditor.rs:1005`, `.vizier/.snapshot:19`).
- When the outcome.v1 JSON work lands (tracked separately under the Outcome summaries/IO-contract threads), ensure the same token-usage fields used in `SessionTokenUsage` are surfaced in the standardized Outcome schema so snapshot claims about per-outcome `token_usage` blocks hold for both human epilogues and machine-oriented outputs.

Narrative changes this run: none (review-only; no repository artifacts were modified).
