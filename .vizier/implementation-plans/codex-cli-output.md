---
plan: codex-cli-output
branch: draft/codex-cli-output
status: draft
created_at: 2025-11-16T01:04:50Z
spec_source: inline
---

## Operator Spec
we need to better display what codex is doing. the --json output is insightful and gives everything we could need--we just need to make better use of it. see ~/rust/codex for the source code on it. additionally, on each update, we should display a new line. it'd be nice to see a history of what's going on. and finally, we should extend the same treatment to the vizier approve outputs--everything should looks similar in this regard.

## Implementation Plan
## Overview
Improve how Vizier surfaces Codex activity so operators can follow every phase of a run. Today the spinner/line-overwrite UX hides earlier updates and `vizier approve` still streams raw Codex stdout. By consuming the existing `codex exec --json` event stream and rendering each event as its own log line, we can give maintainers a chronological history for asks/saves/drafts and mirror that same treatment when Codex implements plans during `vizier approve`. This primarily affects CLI users monitoring Codex-backed commands and reviewers who depend on consistent approve output to audit automation.

## Execution Plan
1. **Codex event contract audit**
   - Inspect the `~/rust/codex` repo (focus on its `events` JSON schema) to catalog the event types/fields we can leverage (e.g., phase names, labels, progress percentages, file hints).
   - Update `vizier-core/src/codex.rs` to preserve any additional metadata we need for rendering (timestamps, nested `data` objects) instead of discarding them in `summarize_event`.
   - Document the subset of event fields Vizier will render so future Codex changes don’t silently break the CLI history.

2. **Progress history renderer**
   - Introduce a renderer (likely in `vizier-core::display` or a new `progress.rs`) that turns each Codex event into a structured log entry (`[codex] <phase> — <message>`). It should:
     - Respect verbosity flags (`-q` suppresses everything; `-v/-vv` can add debug payloads).
     - Emit one newline per event while retaining earlier lines (no spinner overwrite), satisfying the “history” requirement.
     - Fall back gracefully when ANSI is disabled or stdout/stderr aren’t TTYs (align with the stdout/stderr contract thread).
   - Extend `Status`/`ProgressHook` plumbing so hooks can deliver structured events (not just strings) to the renderer, while non-Codex callers can keep the existing spinner behavior.

3. **Wire Codex-backed CLI flows to the renderer**
   - Update `Auditor::llm_request_with_tools` and `display::call_with_status` so Codex runs executed during `vizier ask`, `vizier save`, `vizier draft`, etc., publish their event stream through the new renderer instead of the transient spinner.
   - Ensure the renderer interops with quiet/protocol/--json modes: quiet suppresses logs, protocol mode still emits NDJSON-only output on stdout, and all human-readable history stays on stderr.
   - Surface the accumulated history in error cases too (e.g., if Codex aborts mid-run, the last rendered event should note the failure).

4. **Adopt the same treatment for `vizier approve` (and other passthrough Codex calls)**
   - Change `apply_plan_in_worktree` (and any other approve-specific Codex callers such as `try_auto_resolve_conflicts`) to request `RequestStream::Status` / `CodexOutputMode::EventsJson` instead of passthrough mode.
   - Feed those events into the shared renderer so `vizier approve <slug>` prints the same chronological `[codex]` log as asks/saves.
   - Verify that auxiliary plan steps (`refresh_plan_branch` in `vizier merge`) already reuse the common path so they automatically inherit the new logging; if not, route them through the renderer as well.

5. **Regression tests and docs**
   - Add unit coverage for the event-to-line formatter (given sample payloads from `~/rust/codex`, assert the rendered text).
   - Extend integration tests in `tests/src/lib.rs` to ensure commands like `vizier approve` emit multiple `[codex]` lines in non-TTY mode and suppress them under `--quiet`.
   - Update the snapshot/README/docs workflow section to mention that Codex progress now appears as discrete log lines so operators know what to expect.

## Risks & Unknowns
- Codex event schema could evolve; without strong documentation/tests, future changes might break the renderer. Mitigation: codify the supported fields and add fixture tests.
- Printing every event could be noisy on large runs; need to balance verbosity with usability (maybe collapsing very frequent heartbeat events).
- Switching `vizier approve` away from passthrough might hide raw stderr that operators relied on for debugging; consider preserving a verbose mode or logging unrecognized events at `-v`.

## Testing & Verification
- Unit tests for the new renderer to confirm formatting, quiet-mode suppression, and ANSI gating.
- Integration tests (CLI) that run a mocked Codex session and assert the emitted stderr contains per-event lines for both ask/save and approve workflows; include variants for quiet and non-TTY to ensure compliance with the stdout/stderr contract.
- A regression test (could use the existing mock Codex feature) verifying that `vizier approve` no longer defaults to passthrough but still surfaces progress history.

## Notes
- Coordinate with the stdout/stderr contract and Outcome threads: the new renderer must not interfere with standardized Outcome epilogues or protocol mode, and any documentation updates should cross-link to those TODOs.

Outcome: Drafted plan for codex-cli-output progress history refactor.
