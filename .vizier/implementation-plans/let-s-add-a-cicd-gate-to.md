---
plan: let-s-add-a-cicd-gate-to
branch: draft/let-s-add-a-cicd-gate-to
status: draft
created_at: 2025-11-17T16:06:06Z
spec_source: inline
---

## Operator Spec
let's add a cicd gate to the merge command. on merge, we try the cicd (represented by a shell script file, configurable by flag or file). if this fails, by default we just notify the user with the script output. this can be configured for automatic resolution by the agent--on cicd failure, take the output and prompt the agent with it and context on what's going on (implementing X plan, failed during CICD check on merge, here's the output)--using a flag or through a config file, along with another flag for the number of retries before the agent resolution attempts stop

## Implementation Plan
## Overview
We need to add a CI/CD gate to `vizier merge` so that operators can wire a repo-defined shell script into the merge workflow, surface failures with captured logs, and optionally let the merge agent attempt automated fixes using the script output as context. This touches the merge choreography documented in `docs/workflows/draft-approve-merge.md` and the agent workflow orchestration thread in the snapshot. Primary users are operators (and downstream agents) who rely on `vizier merge` to deliver merge-ready branches; they need deterministic gate behavior, clear UX when checks fail, CLI/config knobs to point at their CI script, and autop-run levers that reuse existing Codex capabilities.

## Execution Plan
1. **Model CI/CD gate configuration + CLI overrides**
   - Extend `vizier-core/src/config.rs` with a `MergeConfig` struct (hung off `Config`) that holds `cicd_gate` settings: script path, whether auto-resolution is enabled, and max retry attempts. Parse `[merge.cicd_gate]` tables from config files (TOML/JSON) alongside the existing `review.checks` parsing and expose defaults (no script configured implies gate disabled).
   - Update `vizier-cli/src/main.rs:288`’s `MergeCmd` to expose explicit flags:
     - `--cicd-script <PATH>` to override the script file per invocation.
     - `--auto-cicd-fix/--no-auto-cicd-fix` (or similar) to force-enable/disable agent remediation regardless of config.
     - `--cicd-retries <N>` to tune the retry budget.
   - Expand `MergeOptions` (`vizier-cli/src/actions.rs:302`) to carry an effective `CicdGateOptions` struct that merges CLI overrides with config defaults while validating inputs (existing script path must exist/exec). Honor `GlobalOpts.push` (still true only when gate passes) and make sure CLI warnings explain when auto-resolution flags are ignored because the merge backend isn’t Codex.

2. **Insert gate execution into the merge workflow**
   - Implement a helper (e.g., `run_cicd_gate_for_merge`) near `run_merge` in `vizier-cli/src/actions.rs:1261` that:
     - Skips work when no script is configured.
     - Runs the configured script (`Command::new("sh").arg(script)` or exec) from the repo root, captures exit status, stdout/stderr (reusing `clip_log` behavior), and duration.
     - Emits human-friendly logs to stderr/stdout: announce the command, print failures, and, on success, note completion before proceeding.
   - Call this helper before `finalize_merge` in both code paths:
     - Right after `commit_ready_merge` returns (before the existing `finalize_merge(...)` call).
     - Inside the early-return path when `try_complete_pending_merge` yields `PendingMergeStatus::Ready` (currently lines ~1385 & ~1320) so resumed merges still honor the gate.
   - Gate semantics: if the script exits non-zero after all remediation attempts are exhausted, exit `vizier merge` with an error, skip `finalize_merge`, and leave the merge commit & draft branch intact so the operator can investigate manually. Document this behavior for reviewers.

3. **Auto-resolution loop using the merge agent**
   - Introduce a Codex prompt builder (e.g., `codex::build_cicd_failure_prompt`) in `vizier-core/src/codex.rs` that accepts plan slug/branch, target branch, script name, exit code, and captured stdout/stderr. Structure it like the existing review/merge-conflict prompts: include `<codexBounds>`, snapshot/TODO context, `<cicdContext>` with metadata, and `<gateOutput>` containing truncated logs.
   - In `run_cicd_gate_for_merge`, when a script run fails and auto-resolution is enabled **and** `agent.backend == Codex`, loop until the retry budget is exhausted:
     - Use `Auditor::llm_request_with_tools` (same plumbing as `refresh_plan_branch`) with the new prompt to let Codex edit the repo (still on the target branch that now includes the merge commit). Keep track of session logs.
     - After Codex returns, if the working tree diff is non-empty, stage all changes and commit them with a descriptive message (e.g., `fix: address CI gate failure for plan {slug} (attempt {i})` via `CommitMessageBuilder`). If Codex changed nothing, log that and proceed to the next retry immediately without committing.
     - Rerun the script; if it passes, break the loop and continue to `finalize_merge`.
   - Add a `#[cfg(feature = "mock_llm")]` hook so CI remediation can be tested deterministically (e.g., when an env var like `MOCK_CICD_EXPECTS_FIX_FILE` is set, the mock branch will create that file so the gate passes on the next attempt).
   - Ensure we collect and surface script output and agent attempt counts in the final printed summary (maybe append to the final `println!` in `run_merge`) and propagate session/token usage logging.
   - If auto-resolution is disabled or we’re on the wire backend, we simply emit the failure logs once and exit early.

4. **Documentation + UX updates**
   - README Core Commands and the plan workflow doc (`docs/workflows/draft-approve-merge.md`) need new sections describing the CI/CD gate: how to configure `[merge.cicd_gate]`, which CLI flags override it, expectations when it fails, and how auto-resolution behaves (including the requirement for Codex). Make sure the doc clarifies that `vizier merge` now runs `{script}` between the merge commit and branch deletion/push, and that failed gates exit non-zero while leaving the merge results for inspection.
   - Add a configuration snippet to README/AGENTS or the config section showing `[merge.cicd_gate] script = "./scripts/run-ci.sh" auto_resolve = true max_attempts = 2`.
   - Update inline CLI help strings in `vizier-cli/src/main.rs` to describe the new flags, and add release-note style messaging in `display::info` lines when the gate runs or auto-resolution kicks in.

5. **Testing + validation harness**
   - Extend integration tests in `tests/src/main.rs` or `tests/src/lib.rs` to cover:
     - Happy path: configure a script that writes a marker file and exits 0; assert the marker exists after `vizier merge --yes --cicd-script ...` and the command still succeeds (branch deletion + plan removal still happen).
     - Failure path without auto resolution: configure a script that exits 1; assert `vizier merge` exits non-zero, prints clipped output, does **not** delete the draft branch, and leaves the merge commit (so `git log` shows it) — document this expectation in assertions.
     - Auto-resolution path: configure a script that fails until a sentinel file exists, and use the `mock_llm` hook to create that file during the first remediation attempt so the gate passes on retry; assert the final output shows the number of attempts and that an extra commit was created with the generated message.
   - Add unit tests for parsing `[merge.cicd_gate]` config (both TOML and JSON) in `vizier-core/src/config.rs`.
   - Where practical, cover edge cases: invalid script path, retries set to 0, auto-on when backend is wire (should warn + skip auto).
   - Document manual verification: run `cargo fmt`, `cargo clippy`, and targeted integration tests (`cargo test -p vizier-cli --features mock_llm -- merge_cicd_gate::*` once added).

## Risks & Unknowns
- **Branch state after gate failure**: The plan leaves the merge commit (and any auto-fix commits) in-place but reports failure. If stakeholders expect the merge to be fully rolled back when CI fails, we’ll need to adjust to a reset-based strategy. Calling that tension out early with reviewers will keep expectations aligned.
- **Agent capability requirement**: Auto-resolution leans on Codex editing the target branch. Repos that lock `agents.merge` to the wire backend won’t get auto-fixes; we’ll warn and skip, but users must confirm that degraded behavior is acceptable.
- **Script environment assumptions**: The gate just shells out inside the repo root. If orgs require custom env vars, they’ll need to manage them externally (e.g., wrapper scripts); plan notes should mention that nuance.
- **Test determinism**: We rely on a `mock_llm` stub to fake Codex fixes so integration tests can assert retries. Designing that stub carefully is important; otherwise tests could be flaky.

## Testing & Verification
- `cargo test -p vizier-core config::tests::test_merge_cicd_gate_*` to confirm config parsing/new defaults.
- `cargo test -p vizier-cli merge_cicd_gate_*` (unit tests around the helper loop, script output clipping, auto-resolution gating).
- Repository-level integration tests (`cargo test --release --features mock_llm merge_cicd_gate` via `tests/src/main.rs` harness) covering success, failure, and auto-fix flows; verify draft branch deletion behavior and commit counts.
- Manual sanity pass: run `vizier merge <plan> --cicd-script ./scripts/pass.sh` and `--cicd-script ./scripts/fail.sh --auto-cicd-fix --cicd-retries 2` inside `test-repo-active` to observe the CLI epilogue, session logs, and branch state.

## Notes
- Coordinate with documentation reviewers to ensure README + workflow doc copy stays within the “first screenful” guidance and references `AGENTS.md` only where relevant.
- Once the gate lands, follow the snapshot TODO (`todo_agent_command_cicd_gate.md`) to mark its acceptance criteria as satisfied or note any deferred items (e.g., hooking gate metadata into outcome summaries when that subsystem ships).
