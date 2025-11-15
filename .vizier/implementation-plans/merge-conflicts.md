---
plan: merge-conflicts
branch: draft/merge-conflicts
status: implemented
created_at: 2025-11-15T06:57:57Z
spec_source: inline
implemented_at: 2025-11-15T08:58:56Z
---

## Operator Spec
we need something to deal with merge conflicts when vizier merge is run. i'm thinking about two primary options--defer to the user, or a flag for an auto-merge-resolution which relies on codex + a new prompt for doing so

## Implementation Plan
## Overview
- Merge conflicts during `vizier merge` currently bubble up as a git2 error that still references `vizier approve`, leaving operators stranded without a compliant way to finish the metadata-rich merge. The remaining steps of the `draft → approve → merge` arc need conflict-aware guardrails to keep gates, plan docs, and Outcome reporting intact.
- This change adds two sanctioned paths when the merge base diverges: a manual “defer to the operator” workflow that preserves Vizier’s plan metadata, and an opt-in Codex-backed auto-resolution mode that can attempt to reconcile conflicts inside the repo boundary. The work directly supports the Snapshot threads around commit gates, outcome reporting, and agent workflow orchestration.
- Impacted users: anyone landing a drafted plan via `vizier merge`, especially in active repos where `draft/<slug>` lags the target branch.

## Execution Plan
1. **Expose conflict-handling levers on the CLI**
   - Extend `vizier-cli/src/main.rs` + `MergeOptions` with a `MergeConflictStrategy` enum. Default keeps today’s behavior (abort on conflicts) but prints actionable guidance; add a `--auto-resolve-conflicts` (or `--on-conflict codex`) flag to opt into Codex resolution.
   - Thread the enum through `run_merge` so the strategy decision happens alongside other merge options (target branch detection, `--delete-branch`, `--note`, etc.).
   - Update `vizier merge --help` (and README Core Commands) to document the new flag, default behavior, and expectations about git state when conflicts are left for the user.

2. **Refactor the merge helper to report conflicts instead of panicking**
   - Split `vizier-core/src/vcs.rs::merge_branch_no_ff` into two layers:
     1. `prepare_merge` that performs the libgit2 merge calculation once, returning either a clean tree + commit parents or a structured `MergeConflict` payload (files with conflicts, ancestor/head/source SHAs, suggested merge message).
     2. `commit_merge` that writes the tree + commit when `prepare_merge` reports `Clean`.
   - When conflicts arise, materialize the merged index into the working tree (so both manual users and Codex see conflict markers) and persist a resume blob (e.g., `.vizier/tmp/merge-conflicts/<slug>.json`) capturing plan slug, target/source refs, and the merge commit message. Ensure rerunning `vizier merge <slug>` detects this sentinel and reuses it instead of re-merging.
   - Plumb `display::info/warn` messaging so operators immediately know which files conflicted and where the resume metadata lives. Maintain TTY/no-ANSI rules per the stdout/stderr contract thread.

3. **Manual resolution workflow (`strategy = Manual`)**
   - On the first conflict, show clear instructions: the repo is now checked out to `<target>`, conflict markers are present, run `git status`/`git add` to resolve, then rerun `vizier merge <slug>` when clean. Exit with a non-zero status so automated scripts notice the conflict.
   - On rerun, detect the stored conflict metadata + clean index. Verify no remaining `git status --porcelain` entries of type `U*`; if any remain, bail with guidance.
   - Once the working tree is conflict-free, reuse the stored merge message (still embedding the plan metadata, implementation-plan excerpt, optional `--note`, etc.), create the merge commit, remove the sentinel, and continue with the existing branch deletion/push/token reporting steps. This guarantees metadata parity regardless of who resolved the conflict.

4. **Codex-backed auto-resolution workflow (`strategy = Codex`)**
   - Add a new prompt constant (e.g., `MERGE_CONFLICT_PROMPT`) under `vizier-core/src/lib.rs` that tells Codex its only job is to resolve the current merge conflicts, remove markers, and leave the repo staged for Vizier to commit. The prompt should recap:
     - The conflicting files (from `git diff --name-only --diff-filter=U`) and branch names.
     - Snapshot/TODO context so Codex understands the project norms without rewriting unrelated files.
     - Bounds identical to other Codex flows (stay in repo, no CLI recursion, summarize edits).
   - In `vizier-cli/src/actions.rs::run_merge`, when conflicts occur and the strategy is Codex:
     - Stream a progress status (“Resolving conflicts with Codex…”), invoke the Codex backend via `vizier-core/src/codex.rs::run_exec` with the new prompt, and wire the request through the existing config (bin/profile/bounds). Capture token usage for the final Outcome line.
     - After Codex exits, re-check for conflicts. If any remain, leave the sentinel + conflict state intact and return an error that nudges the operator to finish manually (the flag only attempts once per run).
     - If Codex succeeds (no conflicts, staged files ready), call the same resume path as the manual workflow to create the merge commit, then clean up sentinel directories.

5. **Outcome + UX polish**
   - Adjust `run_merge` messaging so every exit path still reports a factual Outcome (clean merge, conflict left for manual, Codex failed, Codex succeeded). This keeps us aligned with the Outcome Summaries and stdout/stderr threads.
   - Clean the conflict sentinel + temporary worktree even when Codex/manual flows fail; otherwise subsequent merges might be blocked. Hook the cleanup into `PlanWorktree::cleanup` or a new helper under `.vizier/tmp/`.
   - Update README Core Commands + any user-facing error text to mention the conflict strategies and the expectation that plan metadata is still enforced via `vizier merge`.

## Risks & Unknowns
- **State drift if crashes happen mid-conflict**: Leaving `.git/MERGE_HEAD` plus a Vizier sentinel risks trapping operators if cleanup fails. Mitigate by validating/cleaning stale sentinels on startup and documenting manual recovery.
- **Codex capability limits**: Auto-resolution may still fail on large or binary conflicts. We’ll default to the manual path and clearly report when Codex could not resolve so operators know they must intervene.
- **Prompt correctness**: The new merge-conflict prompt needs enough context/context size guardrails to avoid Codex rewriting unrelated files. We should pilot with a conservative instruction set and reuse existing bounds.
- **Testing complexity**: Integration tests run against a mock Codex backend, so we must design seams that let us assert strategy switching without invoking a real model. The plan above assumes we can stub codex responses or skip auto tests when the flag is unused.

## Testing & Verification
- **Unit tests (Rust)**:
  - Cover the new `MergeConflictStrategy` parsing + default selection in `vizier-cli`.
  - Exercise the refactored `prepare_merge` helper to ensure we correctly identify conflict file sets and that the stored metadata serializes/deserializes as expected.
  - Validate `build_merge_conflict_prompt` contents (includes bounds, snapshot, conflict list) under the `mock_llm` feature.
- **Integration tests (`tests/src/main.rs`)**:
  - Happy path unchanged: existing merge tests still pass when no conflicts.
  - Manual workflow: fabricate conflicting edits on the target + draft branch, run `vizier merge` (expect non-zero, conflict instructions, sentinel file), resolve the conflicts inside the fixture repo, rerun `vizier merge` and assert the merge commit embeds plan metadata and the sentinel is gone.
  - Codex flag smoke test: with `mock_llm` enabled, mark the merge command with `--auto-resolve-conflicts` and assert we attempt the Codex call (check stdout for the progress label / mock usage) and finish the merge automatically once the mock backend reports success.
  - Failure fallback: simulate Codex returning an error (using the mock backend) and verify the command returns non-zero while leaving the conflict state intact for manual cleanup.
- **CLI UX checks**: manual verification that `vizier merge --help` lists the new options, README text renders correctly, and conflict warnings respect `--quiet`/TTY rules.

## Notes
- Coordinate with documentation owners to fold the conflict strategy into the broader agent workflow guidance (README + any future APPROVE/MERGE docs).
- If we add more workflow checkpoints later (e.g., Protocol mode outcome events), reuse the conflict sentinel metadata so other commands can surface “merge awaiting manual resolution” in their Outcome summaries.
