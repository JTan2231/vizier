---
plan: we-re-missing-a-review-functiona
branch: draft/we-re-missing-a-review-functiona
status: draft
created_at: 2025-11-16T23:16:53Z
spec_source: inline
---

## Operator Spec
we're missing a review functionality--vizier review <slug> -> outputs a critique mentioning test or compile failures, or gaps against hte implementation plan, or etc. this should be a separate command, with a y/n user for whether to have codex address the comments

## Implementation Plan
## Overview
- Add a dedicated `vizier review <slug>` command so operators can request an auditable critique of a `draft/<slug>` branch before merge. The review must synthesize build/test results, alignment with `.vizier/implementation-plans/<slug>.md`, and any gaps Codex sees versus the Operator Spec and snapshot threads.
- The command should live in the same plan workflow as `vizier draft → vizier approve → vizier merge`, giving humans and downstream agents a clear checkpoint for “code reviewed and ready”. This closes the gap noted in the Agent workflow orchestration and compliance snapshot threads.
- After surfacing the critique, prompt the operator (default interactive y/N) on whether to hand Codex the review notes to address the findings on the plan branch. This keeps automation optional while supporting continuous agent runs.

## Execution Plan
1. **CLI surface + plan metadata plumbing**
   - Extend `vizier-cli/src/main.rs` to add a `review` subcommand (`vizier review <plan>`) with options: positional slug, `--branch` override, `--target` for diff context, `-y/--yes` to auto-apply fixes without prompting, and `--review-only` (or inverse `--apply-fixes`) so operators can skip the Codex fix-up stage explicitly.
   - Wire the new command into `vizier-cli/src/actions.rs` with a `run_review(ReviewOptions)` entry point alongside `run_draft/approve/merge`. Reuse `plan::PlanBranchSpec::resolve` to locate the slug, branch, and target. Gate the command behind the Codex backend (like `approve`) and require a clean working tree before touching plan branches so the main checkout stays untouched.
   - Update `plan::PlanMetadata` helpers if needed to expose review-related timestamps/statuses (`status: review-requested`, `reviewed_at`, etc.) so the CLI can mark when review artifacts exist. Ensure `PlanSlugInventory` prints review status when listing pending plans.

2. **Temporary worktree + review context capture**
   - Create a disposable `PlanWorktree` (purpose “review”) checked out to the plan branch and guard `cwd` via `WorkdirGuard` just like `approve`. Validate that `.vizier/implementation-plans/<slug>.md` is present and warn if the branch lacks post-approve commits.
   - Introduce a review check runner that executes repository-defined commands before Codex sees anything. Default to running `cargo check --all --all-targets` and `cargo test --all --all-targets` when `Cargo.toml` exists; allow overrides via new config (e.g., `[review.checks] commands = ["npm test", "cargo clippy -- -D warnings"]`). Capture stdout/stderr, exit codes, and durations for each command.
   - Compute diff context against the target branch (reuse `PlanBranchSpec::diff_command`/`git diff --stat target...branch`) plus any uncommitted files inside the worktree. Persist these facts in memory for prompt assembly and print a concise summary to stdout/stderr so humans can see failures even before Codex comments on them.

3. **Review prompt + artifact storage**
   - Add a `REVIEW_PROMPT` constant to `vizier-core/src/lib.rs` that instructs Codex to (a) read the stored plan, (b) inspect the diff summary, (c) weigh captured test/build logs, and (d) produce a markdown critique with sections like “Plan Alignment”, “Tests & Build”, “Outstanding TODO/Thread Impacts”, and “Action Items”.
   - Implement `codex::build_review_prompt(plan_slug, branch, target_branch, plan_doc, diff_summary, check_results)` to weave snapshot text, active TODO threads, plan metadata, and the collected check outputs into the prompt. This should parallel `build_implementation_plan_prompt` and respect the repo-local bounds prompt.
   - Run Codex via `Auditor::llm_request_with_tools_no_display` (no tool execution, review only) and save the returned critique to `.vizier/reviews/<slug>.md` with front-matter `{plan, branch, target, reviewed_at, reviewer}`. Update the corresponding plan document’s front-matter (e.g., `status: review-requested` → `status: review-ready`, `last_reviewed_at`) so downstream tooling knows review artifacts exist. Surface the review path, session log path, diff command, and check verdicts in the CLI’s Outcome line.

4. **Optional Codex fix-up flow**
   - After printing the critique, present `Apply suggested fixes on draft/<slug>? [y/N]` unless the operator passed `--yes` or `--review-only`. If accepted, reuse the existing Codex editing pipeline inside the same worktree, but feed it an instruction that references both the plan document and the saved review file so Codex knows which gaps to close.
   - Stage and commit any edits the fix-up run produces using `CommitMessageBuilder` with a `fix:` or `chore:` header (something like “fix: address review feedback for <slug>”), attach the review artifact path in the commit message metadata, and optionally re-run the configured checks if time allows. Update the plan doc to `status: review-fixes-in-progress` before the run and `status: review-addressed` afterward.
   - When fixes are skipped, leave the branch untouched but keep the review artifact and status so humans know further work is needed.

5. **Docs, UX, and workflow integration**
   - Update `README.md` Core Commands and workflow descriptions to introduce `vizier review <plan>` (purpose, default behavior, key flags, learn-more anchor pointing to the workflow doc). Make it clear that `approve` implements, `review` critiques/tests/optionally fixes, and `merge` lands the branch.
   - Expand `docs/workflows/draft-approve-merge.md` into “draft → approve → review → merge”, documenting prerequisites, artifacts, where review files live, how the optional fix prompt works, and how to resume after changes requested. Tie this stage back to the Agent workflow orchestration thread so multi-agent runs have an auditable checkpoint.
   - Refresh shell-completion metadata (`plan::PlanSlugInventory`) and `vizier list` output so review status and artifact paths are visible. Ensure session logging/outcome summaries mention the review session path and `review_file` to meet the Outcome summaries + session logging requirements.

## Risks & Unknowns
- **Check execution variability**: Running `cargo test`/`cargo check` (or user-defined commands) on large repos may be slow or require services not available in the disposable worktree. Mitigate by allowing configurable command lists + `--skip-checks`/`--checks-only` toggles, and surfacing failures even when commands cannot run (so Codex still has context).
- **Codex fix stage scope creep**: Automatically addressing review comments risks diverging from the stored implementation plan or re-opening TODO threads. Keep the fix instruction constrained to the saved review file + plan summary and require commits to go through the existing Pending Commit gate when merging.
- **Plan metadata churn**: Updating plan/review statuses introduces more front-matter fields; need to ensure older plan files without those fields still parse and that `PlanSlugInventory` tolerates missing review info.

## Testing & Verification
- **Integration tests (tests/src/lib.rs)**: Add scenarios that (a) draft + approve a plan, run `vizier review <slug> --review-only` and assert the review artifact exists, (b) simulate a failing check script (e.g., inject a shell script under `test-repo/ci/check.sh`) to ensure output is captured and reflected in CLI stdout, and (c) run with `--yes` to confirm Codex fix-up commits land on the plan branch without touching the main checkout.
- **Prompt/unit tests**: Add coverage for `codex::build_review_prompt` to verify snapshot, TODO threads, plan metadata, diff summary, and check outputs all appear in the generated prompt (use a mock context similar to existing prompt tests).
- **Plan metadata tests**: Extend `plan::tests` to ensure new status/timestamp fields are parsed and serialized correctly, and that setting review status updates front-matter idempotently.
- **Config tests**: Add config parsing tests for the new `[review]` section (default commands, overrides, skip flags) to avoid regressions in `vizier-core/src/config.rs`.
- **Outcome/session assertions**: Update CLI integration tests to assert that the review command prints the session path and review file path, keeping the Outcome summaries + session logging requirements intact.

## Notes
- Summary: drafted a plan for adding a `vizier review` command with automated check capture, Codex-powered critiques, optional fix-up prompting, artifact storage, and documentation/test coverage so the draft → approve → merge workflow gains an explicit review gate.
