---
plan: single-commit-1
branch: draft/single-commit-1
status: draft
created_at: 2025-11-20T15:57:10Z
spec_source: inline
---

## Operator Spec
replace the current dual-commit choreography with a single commit per Vizier action that captures all intentional artifacts—i.e., tracked code plus canonical narrative files like .vizier/.snapshot and TODO threads—while
      continuing to treat plan docs, .vizier/tmp/*, and session logs as non-committable scratch.

## Implementation Plan
## Overview
- Vizier currently finalizes every assistant interaction with two commits: `vizier-core/src/auditor.rs:533-607` auto-commits `.vizier` artifacts, and each CLI command then creates a second code commit (e.g., `vizier-cli/src/actions.rs:687-808` for `save`, `vizier-cli/src/actions.rs:2751-2845` for plan apply). This splits intentional edits across history even though the operator spec now demands a single auditable commit per action.
- We need to refactor auditor/file-tracking so narrative updates stay staged until the surrounding command commits everything once, while making sure only canonical story files (`.vizier/.snapshot`, thread TODOs) ride along—plan docs, `.vizier/tmp/*`, and session logs must keep living as scratch.

## Execution Plan
1. **Model canonical `.vizier` change sets inside the auditor layer**
   - Replace the auto-commit branch of `Auditor::finalize` in `vizier-core/src/auditor.rs:533-607` with a structure (e.g., `NarrativeChangeSet`) that captures the pending `.vizier` paths plus the LLM-generated summary text but leaves files uncommitted.
   - Extend `AuditResult` to carry that change set alongside the session artifact so callers know when narrative files need to be staged, and preserve the current `Clean/Pending` states for `--no-commit`.
   - In `vizier-core/src/file_tracking.rs:52-145`, drop `commit_changes` and instead expose helpers such as `pending_paths()` and `clear_tracked(&paths)` that (a) enumerate tracked `.vizier` files and (b) clear only after the caller confirms the shared commit succeeded. Apply a filter so only `.vizier/.snapshot` plus root-level `.vizier/*.md` thread files are returned; explicitly exclude `.vizier/implementation-plans/`, `.vizier/tmp/`, `.vizier/sessions/`, and other scratch subtrees per the spec.
   - Acceptance: calling `Auditor::finalize` with narrative edits reports `AuditState::Committed` plus a change set without touching Git history; `--no-commit` still reports `Pending` and leaves trackers untouched.

2. **Teach the commit builder/staging code to fuse narrative + code metadata**
   - Enhance `CommitMessageBuilder` in `vizier-core/src/auditor.rs:1213-1340` with an optional narrative section so code commits can append a concise “Narrative updates” stanza (populated from Step 1 summaries) while pure narrative commits still default to the existing `VIZIER NARRATIVE CHANGE` fallback.
   - Add a helper (ideally near the shared CLI utilities in `vizier-cli/src/actions.rs:640-820`) that accepts the new change set and stages only the canonical `.vizier` files before we run `git add` for the rest of the tree. This helper should no-op when no narrative edits exist and should know how to clear the tracker only after the commit completes.
   - Acceptance: building a commit with both code and `.vizier` edits yields one git commit whose body contains both the code summary and a short narrative section; staging helper skips plan docs/sessions even if Codex touched them.

3. **Apply the single-commit path across CLI workflows**
   - `vizier ask` (`vizier-cli/src/actions.rs:913-987`): after `Auditor::finalize`, call the new staging helper and produce a single narrative commit when auto-commit is enabled; `--no-commit` leaves both code and `.vizier` files dirty with clear messaging.
   - `vizier save` (`vizier-cli/src/actions.rs:687-808`): instead of letting the auditor create a discrete `.vizier` commit, pass the narrative summary into the commit builder that already handles code diff summaries so we emit one commit when either domain changed. Skip staging when neither domain changed.
   - Draft-plan implementation and review flows—`apply_plan_in_worktree` (`vizier-cli/src/actions.rs:2751-2845`), fix-ups (`vizier-cli/src/actions.rs:2620-2712`), CI/CD remediation (`vizier-cli/src/actions.rs:1728-1817`), and review critique commits (`vizier-cli/src/actions.rs:2319-2385`)—should all call the helper before `vcs::stage(Some(vec!["."]))?` so the resulting plan-branch commit contains both code and `.vizier` updates. Confirm that `push_origin_if_requested` still runs once per commit.
   - Ensure `Auditor::commit_audit` (used in `refresh_plan_branch` at `vizier-cli/src/actions.rs:2848-2890`) now just persists the session log and returns change metadata; consumers must explicitly stage `.vizier` files before committing.
   - Acceptance: running `vizier save` or `vizier approve` produces exactly one new commit even when both code and `.vizier/.snapshot` changed, and `git log --stat` shows the combined file set.

4. **Update docs/tests to reflect the new flow**
   - Integration tests in `tests/src/lib.rs` (e.g., the save/approve coverage near the existing helpers around lines 1-200) should assert that each command increments the commit count by one and that `.vizier/.snapshot` plus code files appear in the same commit diff. Add regression coverage for `--no-commit` leaving everything dirty.
   - Adjust README (`README.md:70-140`) and the workflow doc (`docs/workflows/draft-approve-merge.md:1-160`) to mention “single commit per Vizier action” and clarify that plan docs/`./.vizier/tmp` remain scratch even though narrative files are staged automatically.
   - Acceptance: tests fail if multiple commits are generated or if plan docs sneak into the combined staging set; documentation matches the new behavior.

## Risks & Unknowns
- Spec vs workflow tension: plan branches currently commit `.vizier/implementation-plans/…`; the operator spec calls them scratch. We’ll keep plan-doc commits for the plan workflow but highlight this assumption for confirmation; otherwise a broader docs/story change is required.
- `FileTracker` may already be referenced by tool implementations; exposing `clear()` could accidentally wipe pending edits if misused. Need tight ownership semantics so only the new staging helper clears trackers after successful commits.
- Combining code and narrative summaries might overwhelm the commit body or confuse downstream automation expecting separate commits; we should align formatting with the Git hygiene thread to avoid regressions.
- `--no-commit` workflows rely on the tracker to remember pending `.vizier` files; ensuring they survive across command invocations without double-staging is subtle.

## Testing & Verification
- Extend integration coverage in `tests/src/lib.rs` to run `vizier save`, `vizier ask`, `vizier approve`, `vizier review --review-only`, and CI/CD remediation with mocked Codex, then assert:
  - Only one commit was created per command.
  - The commit diff includes both `.vizier/.snapshot` and code files when both changed.
  - `--no-commit` leaves staged/working-tree changes plus the tracker marked dirty.
- Run targeted unit tests for the new `FileTracker` filtering logic (simulate mixed `.vizier` paths and confirm only canonical files are surfaced) and for the augmented `CommitMessageBuilder`.
- Re-run the existing integration suite (`cargo test -p vizier-cli -p vizier-core`, full `tests` crate) to ensure no regressions in draft/approve/merge behavior after the refactor.

## Notes
- Narrative delta: captured a plan to collapse Vizier’s dual commits into a single combined commit per action while filtering out plan-doc/session scratch so code and canonical `.vizier` artifacts travel together.
