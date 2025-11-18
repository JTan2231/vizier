---
plan: reviews
branch: draft/reviews
status: draft
created_at: 2025-11-18T06:01:22Z
spec_source: inline
---

## Operator Spec
vizier review should _not_ leave behind artifacts. anything regarding the .vizier/reviews directory needs removed. for now, vizier review should output its review to the terminal--not a file.

## Implementation Plan
## Overview
Vizier’s current review workflow writes Codex critiques to `.vizier/reviews/<slug>.md` (README.md:72; docs/workflows/draft-approve-merge.md:32,115) and threads that path through `vizier review` (vizier-cli/src/actions.rs:2265-2410) plus the Codex prompt builder (vizier-core/src/codex.rs:414-475). The operator spec now forbids persisting these artifacts; review feedback must stream to the terminal only, and any `.vizier/reviews` directory references need to disappear. We’ll refit the CLI, prompts, docs, and tests so reviews remain auditable through session logs and plan-status updates without leaving Markdown files behind.

## Execution Plan
1. **Inline the review critique instead of writing `.vizier/reviews` files**
   - Remove the `review_rel` path from `perform_review_workflow` and `ReviewOutcome` (vizier-cli/src/actions.rs:2209-2410), delete `write_review_file`, and stop creating/staging `.vizier/reviews`.
   - After Codex responds, print the critique to stdout/stderr in a structured block (e.g., header plus raw Markdown) and retain the text in memory for downstream steps.
   - Update commit behavior: the review commit should now reflect only plan status / metadata changes, and `CommitMessageBuilder`’s author note should reference the session log instead of a file path. Update the `--no-commit` informational messages so they no longer tell operators to inspect `.vizier/reviews`.
   - Adjust the final outcome line printed from `run_review` (vizier-cli/src/actions.rs:1360-1495) so the `review=` field is replaced with an explicit `critique=terminal` (or similar) indicator while keeping check counts, diff command, and session path. Acceptance: running `vizier review` leaves no `.vizier/reviews` on the branch tree, yet the critique text appears in the terminal output and the plan document still moves to `status: review-ready`.

2. **Rework follow-up fix prompts to consume the in-memory critique**
   - Change `apply_review_fixes` (vizier-cli/src/actions.rs:2637-2720) to accept the critique text rather than a file path. Embed that text inside a new instruction wrapper (e.g., `<reviewCritique>…</reviewCritique>`) so Codex still sees the actionable feedback.
   - Update any `with_author_note` strings or log messages inside the fix path to refer to “review critique output” or the current session log, not a file path.
   - Ensure the prompt builder no longer references `.vizier/reviews/<slug>.md`, and adjust `codex::build_review_prompt` metadata (vizier-core/src/codex.rs:414-475) so the plan metadata block omits the `review_file` line entirely. Acceptance: automatic fix-ups still run because the critique is passed directly, and no code path tries to read a non-existent file.

3. **Refresh docs, sample artifacts, and configuration hints**
   - Rewrite the README’s workflow summary (README.md:62-78) and the detailed walkthrough in docs/workflows/draft-approve-merge.md (especially the “High-Level Timeline” and `--no-commit` sections around lines 32-150) so they explain that `vizier review` streams its critique to the terminal/session log instead of writing `.vizier/reviews/<slug>.md`.
   - Update any other references to `.vizier/reviews` (search surfaced at README.md:72 and docs/workflows/draft-approve-merge.md:32,115,137) to describe the new behavior, including instructions about where to find critiques when `--no-commit` is active.
   - Remove the checked-in `.vizier/reviews` directory and its sample files from the repo, since no command should populate it anymore.

4. **Revise tests and helper artifacts**
   - Replace `test_review_produces_artifacts` (tests/src/lib.rs:1188-1252) with a case that asserts (a) the tree for `draft/<slug>` lacks `.vizier/reviews/*`, (b) the review command’s stdout contains expected section headers such as “Plan Alignment,” and (c) the plan document reflects `status: review-ready`.
   - Add/adjust any other tests that referenced the review file path—for example, ensure token suffix tests or fix-up tests don’t assume a stored critique.
   - Run integration/unit suites locally to confirm the new behavior. Acceptance: the revised tests fail under the old behavior and pass once critiques are terminal-only.

## Risks & Unknowns
- Auto-fix flows rely on immediate access to the critique; operators who exit the CLI may need to consult the session log instead. We must confirm session logging already captures the critique so there is still an audit trail.
- `--no-commit` users previously inspected `.vizier/reviews` inside the preserved worktree; now they only have terminal output. We may need to emphasize session log pointers to avoid confusing users.
- Removing the directory may surprise any downstream tooling that scraped `.vizier/reviews`; confirm no other code paths implicitly glob that directory before deleting it.

## Testing & Verification
- Run the updated integration test suite (`cargo test` at repo root) to cover the modified review flow, including the renamed review artifact test and existing approve/merge/CI gate tests.
- Manually run `vizier draft/approve/review` in a sample repo to verify: no `.vizier/reviews` files exist, plan status changes persist, critique text prints once, and automatic fixes still apply when requested.
- Optionally simulate `--no-commit` runs to ensure the CLI messaging correctly explains where critiques live and that the preserved worktree contains only plan doc diffs.

## Notes
- Once critiques no longer land in `.vizier/reviews`, session-log discoverability becomes more important; if reviewers still want durable Markdown, we may need a follow-up thread to expose `vizier review --export` or similar.
