# Draft → Approve → Merge Workflow

This guide explains how Vizier’s plan workflow turns a high-level spec into audited code without disturbing your working tree. Use it whenever you want Vizier (or an external agent) to implement a scoped change on a dedicated branch and then merge it back with a metadata-rich commit.

## High-Level Timeline

1. **`vizier draft <spec>`** — Creates a `draft/<slug>` branch and writes `.vizier/implementation-plans/<slug>.md` inside a disposable worktree based on the primary branch. Your checkout stays untouched.
2. **`vizier approve <slug>`** — Applies the plan on `draft/<slug>` from within another temporary worktree, staging and committing the resulting edits on that branch only.
3. **`vizier merge <slug>`** — Refreshes the plan branch, removes the plan document, and performs a non–fast-forward merge into the target branch with the stored plan embedded under an `Implementation Plan:` block in the merge commit.

At every stage you can pause, review the artifacts, and hand control back to a human maintainer.

## `vizier draft`: create the plan branch

**Prerequisites**
- Codex backend is active (`backend = "codex"`).
- Primary branch is up to date (auto-detected via `origin/HEAD`, `main`, or `master`).

**What it does**
- Derives a slug from your spec (or `--name`) and creates `draft/<slug>` from the detected primary branch.
- Spawns a disposable worktree under `.vizier/tmp-worktrees/<slug>-<suffix>/` and runs Codex there so your current working tree never gets touched.
- Writes the Codex-produced Markdown to `.vizier/implementation-plans/<slug>.md` and commits it on `draft/<slug>` with message `docs: add implementation plan <slug>`.
- Removes the temporary worktree when successful, printing the on-disk plan so you can review it.

**Flags to remember**
- `vizier draft "spec"` — inline operator spec.
- `vizier draft --file SPEC.md` — load spec from disk.
- `vizier draft --name hotfix-foo "..."` — override the slug + branch name.

**How to verify**
```bash
git branch --list 'draft/*'
git log --oneline draft/<slug> | head -n 3
```
Both commands should show the plan commit sitting one commit ahead of the primary branch while your current working tree remains clean.

## `vizier approve`: implement the plan safely

**Prerequisites**
- Clean working tree (enforced by `vcs::ensure_clean_worktree`).
- Codex backend enabled.
- Plan branch (`draft/<slug>` or `--branch`) and target branch (`--target`, otherwise detected primary) exist locally.

**What it does**
- Validates that `draft/<slug>` is based on the current target branch; warns if the branch is behind.
- Creates a temporary worktree `.vizier/tmp-worktrees/<slug>-<suffix>/` checked out to the plan branch and runs Codex against the stored plan document.
- Codex edits `.vizier/.snapshot`, TODOs, and code directly inside that worktree; Vizier stages `.` and commits the changes on the plan branch with the Auditor-provided commit message.
- Your original checkout stays untouched. On success the temp worktree is removed; on failure it is preserved for debugging and the branch keeps whatever Codex staged.

**Flags to remember**
- `vizier approve <slug>` — default flow.
- `vizier approve --list` — dump every `draft/*` branch that is ahead of the target.
- `vizier approve --target release/1.0` — preview and diff against a branch other than the detected primary.
- `vizier approve --branch feature/foo` — when your work diverges from `draft/<slug>` naming.
- `vizier approve -y` — skip the confirmation prompt.

**Git effects**
- Only `draft/<slug>` receives commits. The target branch and your working tree never change.
- Vizier prints `review with "git diff <target>...<draft/<slug>>"` so you can inspect the diff immediately.
- If Codex makes no changes, the command aborts with `Codex completed without modifying files` to avoid empty commits.

## `vizier merge`: land the plan with metadata

**Prerequisites**
- Clean working tree.
- Plan branch contains Codex commits you want to land.
- You’re ready to remove `.vizier/implementation-plans/<slug>.md` from the branch.

**Prep work (handled automatically)**
1. Creates a temporary worktree on the plan branch.
2. Deletes `.vizier/implementation-plans/<slug>.md` from that branch so the plan doc does not land in the target branch.
3. Runs a `vizier save`–style Codex refresh to make sure `.vizier/.snapshot` + TODOs in the plan branch reflect the latest story before merging.
4. Cleans up the worktree.
5. Checks out the target branch locally (switches branches if needed).

**Merge mechanics**
- Builds a merge commit with subject `feat: merge plan <slug>` and a body that only contains
  - An optional `Operator Note: …` line when `--note` is present.
  - An `Implementation Plan:` block that inlines the stored plan document (or a placeholder if the file cannot be read).
- Calls `vcs::prepare_merge` for a non–fast-forward merge. If there are no conflicts, `vizier merge` immediately commits the merge with parents `[target, draft/<slug>]` and prints the resulting SHA.
- When conflicts occur, Vizier writes `.vizier/tmp/merge-conflicts/<slug>.json` with the HEAD/source commit IDs and conflict list, then:
  - With `--auto-resolve-conflicts`, runs Codex inside the repo to try resolving and, if successful, finalizes the merge automatically.
  - Otherwise, instructs you to resolve conflicts manually, stage the files, and rerun `vizier merge <slug>`; Vizier will detect the sentinel JSON and finish the merge once the index is clean.
- Successful merges delete `draft/<slug>` automatically as long as the merge commit contains the branch tip; pass `--keep-branch` to retain the branch locally (legacy `--delete-branch` remains as a compatibility alias but is no longer required).
- `--yes` skips the confirmation prompt, and `--target/--branch` behave like they do for `approve`.

**Post-merge artifacts**
- Merge commit on the target branch titled `feat: merge plan <slug>` with the plan document embedded under `Implementation Plan:` (plus any optional operator note).
- Optional branch deletion (local only).
- `.vizier/tmp/merge-conflicts/<slug>.json` cleaned up automatically when the merge completes.

## Failure & recovery playbook

| Situation | Recovery |
| --- | --- |
| Draft worktree creation fails | Vizier deletes the stub branch unless the plan file was already committed. Re-run `vizier draft` once the error is fixed. |
| `vizier approve` fails mid-run | Temp worktree path is printed; inspect it to salvage partially staged files, then rerun once corrected. The plan branch remains intact. |
| Merge conflicts | Resolve conflicts on the target branch, stage the files, rerun `vizier merge <slug>`. Vizier reuses `.vizier/tmp/merge-conflicts/<slug>.json` to finalize. |
| Need to resume after aborting Git’s merge | As long as the sentinel JSON still exists and `git status` is clean, rerunning `vizier merge <slug>` finishes the in-progress merge without repeating refresh/removal steps. |
| Codex auto-resolution fails | Vizier warns and falls back to manual instructions. Resolve/stage/retry just like a normal Git merge. |

## End-to-end walkthrough

1. **Draft**
   ```bash
   vizier draft --file specs/ingest.md
   ```
   - Output shows `Draft ready; plan=.vizier/implementation-plans/ingest-backpressure.md; branch=draft/ingest-backpressure`.
   - Reviewer opens the printed plan file on the draft branch and edits if needed.
2. **Approve**
   ```bash
   vizier approve ingest-backpressure
   git diff main...draft/ingest-backpressure
   ```
   - Maintainer inspects the diff while staying on their original branch.
   - If satisfied, they push `draft/ingest-backpressure` for further review or proceed locally.
3. **Merge**
   ```bash
   vizier merge ingest-backpressure
   ```
   - Vizier refreshes `.vizier/.snapshot`, removes the plan doc, and merges into the detected primary branch (e.g., `main`).
   - Final output looks like `Merged plan ingest-backpressure into main; merge_commit=<sha>`.
   - Need the branch for a follow-up? Append `--keep-branch` to suppress the default deletion step.

Throughout the process, Outcome lines and Auditor records cite the plan slug, affected files, and any pending commit gates so auditors can trace who approved what. Tie this workflow into the broader agent-orchestration story by referencing this document in `AGENTS.md` or external SOPs when onboarding third-party agents.

## FAQ

**Can I run `vizier approve` without re-drafting?**  
Yes. If you manually edit `.vizier/implementation-plans/<slug>.md` on the draft branch, rerun `vizier approve <slug>` to reapply the plan. Vizier refuses to commit if Codex makes no changes so you can iterate safely.

**What if the draft branch lags far behind the target branch?**  
`vizier approve` prints a warning when `draft/<slug>` is missing commits from the target. You can merge or rebase the draft branch manually before running `vizier merge`, or accept that `vizier merge` may surface conflicts which you’ll resolve with the sentinel workflow described above.

**Does `vizier merge` push to origin?**  
Not automatically. Pass `--push` (global CLI flag) after `approve` or `merge` if you want Vizier to push once the command succeeds, or push manually when you’re ready.

## Architecture docs & agent workflows

- Plan documents live under `.vizier/implementation-plans/` and describe how Codex intends to implement the spec, but they do **not** replace architecture-doc references required by the compliance gate. Keep your architecture doc path handy so `vizier save` can cite it when the plan’s changes land on the primary branch.
- When multiple agents collaborate, use this workflow as the runbook: one operator drafts, another approves, and a third merges. The Auditor already records Outcome lines for each command, so reference those facts (plus the plan slug) inside `.vizier/.snapshot` and TODO updates to keep the narrative threads aligned.
