---
plan: codex-output
branch: draft/codex-output
status: draft
created_at: 2025-11-15T08:38:29Z
spec_source: inline
---

## Operator Spec
when i said route codex's output to stderr, i meant wholesale--pick the thing up and directly route it to stderr with zero additional processing. we shouldn't be seeing just item.completed or the like--we should be seeing codex's output as if we were running the command ourselves. in this regard, i think it would be apt to drop the --json flag from the codex call--we want the output to be human readable, not machine readable here. this may change later. this is in regards to the vizier approve command

## Implementation Plan
## Overview
Operators running `vizier approve` need to see Codex’s human-formatted stream exactly as if they had invoked `codex exec` themselves. Today the command pipes Codex through the JSON event stream, summarizes each event into `[codex] item.completed …`, and never surfaces the raw text. Per the operator spec, we must route Codex stdout directly to the CLI’s stderr path and drop the `--json` flag (while still capturing the final assistant summary for commits). The change is scoped to the Codex-backed `vizier approve` pipeline (`vizier-cli/src/actions.rs:1356` via `Auditor::llm_request_with_tools_no_display`) and will keep other commands on the structured event path. This primarily affects repo operators who review Codex’s work in real time and dovetails with the active stdout/stderr contract thread.

## Execution Plan
1. **Add a Codex output-mode toggle**
   - Extend `vizier-core/src/codex.rs` with an explicit `OutputMode` (e.g., `EventsJson` vs `PassthroughHuman`) attached to `CodexRequest`.
   - When `PassthroughHuman` is selected:
     - Build the command without `--json`; keep `--output-last-message` so we can still read the assistant summary once the run completes.
     - Pipe Codex stdout and stderr straight to the parent process’s stderr using async copies (tee stderr so we still buffer lines for error classification in `classify_profile_failure`).
     - Skip JSON parsing/`CodexEvent` creation and set `TokenUsage` to `None` because the non-JSON stream doesn’t expose usage; other modes remain unchanged.
   - Acceptance: running Codex in passthrough mode prints exactly what `codex exec …` would print (no `[codex]` prefixes), and structured mode behavior/regressions remain untouched.

2. **Teach the Auditor about the new mode**
   - Update `Auditor::llm_request_with_tools_no_display` (`vizier-core/src/auditor.rs:456`) so callers can request the new passthrough mode (perhaps by changing the `request_tx` argument to an enum that differentiates “send status lines” vs “bypass status, stream raw output”).
   - In Codex backend branches, forward the toggle into `codex::run_exec` and skip the `ProgressHook::Plain` channel when passthrough is active.
   - Make sure we still append an assistant message to the transcript using the `--output-last-message` file, and call `Auditor::mark_usage_unknown()` so downstream token reporting prints “unknown” instead of stale values.
   - Acceptance: chat/TUI callers continue to receive progress updates exactly as before, while `vizier approve` will use the passthrough variant yet still produce a valid `wire::types::Message` for auditing.

3. **Wire `vizier approve` to passthrough mode**
   - Remove the manual `spawn_plain_progress_logger` from `vizier-cli/src/actions.rs:1355` and replace it with a direct stderr stream originating from Codex (now handled inside `run_exec`).
   - Keep the surrounding workflow identical (plan metadata fetching, `apply_plan_in_worktree`, commit creation, optional push) so existing integration tests (`tests/src/main.rs::test_approve_*`) keep passing with `mock_llm`.
   - Update any user-facing log lines if necessary to clarify that Codex output now appears unfiltered on stderr; other status/summary prints should remain on stdout per the stdout/stderr contract.
   - Acceptance: invoking `vizier approve <plan>` shows Codex’s native output on stderr without `[codex] …` prefixes, yet the command still succeeds/fails in the same places and prints the existing success summary on stdout.

4. **Documentation & telemetry follow-through**
   - Because passthrough mode cannot extract token usage, ensure `print_token_usage()` at the end of `run_approve` reports “unknown” rather than stale counters (already handled once the Auditor marks usage unknown).
   - Add a short README/`AGENTS.md` blurb or CLI help note if operators need to know why approve now streams raw Codex output (optional unless reviewers request it).
   - Acceptance: reviewers see the new behavior described in release notes/help, and there is no mismatch between actual output and documentation.

## Risks & Unknowns
- **Usage accounting loss**: Removing `--json` means we no longer receive structured usage stats for approve runs; we will rely on the “unknown” path until Codex exposes another channel.
- **Error classification**: We still need to detect profile/auth failures. The passthrough tee must buffer enough stderr text for `classify_profile_failure`; otherwise, fallback behavior could regress.
- **Quiet mode expectations**: Streaming raw Codex output may ignore `-q`. If suppressing Codex output is a requirement later, we will need a more nuanced contract; for now, spec prioritizes fidelity over silence.
- **Future protocol mode**: When protocol mode lands, we must ensure passthrough streaming still honors the “stdout for JSON, stderr for human output” rule, or gate passthrough behind mode detection.

## Testing & Verification
- **Unit-level**: Add tests around any new helper that constructs Codex args to confirm `--json` is omitted in passthrough mode while still present elsewhere (can be pure Rust tests without invoking the binary).
- **Integration (mock LLM)**: Run the existing approval-focused tests (`cargo test --release --features mock_llm test_approve_merges_plan test_approve_keeps_primary_checkout_clean`) to ensure overall workflow stability after the refactor.
- **Manual CLI smoke**: With a real Codex binary configured, run `vizier approve <plan>` and confirm stderr shows the raw Codex stream, stdout still reports success, and token usage prints “unknown”.
- **Failure path**: Manually induce a Codex auth failure (e.g., bogus profile) and verify the stderr tee still surfaces Codex’s error while the CLI catches and reports it cleanly.

## Notes
- This change aligns with the active “Stdout/stderr contract + verbosity” thread by clarifying which stream carries Codex chatter (stderr) vs Vizier outcomes (stdout). Any follow-up work on protocol mode should reuse the new output-mode toggle so both behaviors stay explicit and auditable.
