# Draft → Approve → Review → Merge Workflow

This guide explains how Vizier’s plan workflow turns a high-level spec into audited code without disturbing your working tree. Use it whenever you want Vizier (or an external agent) to implement a scoped change on a dedicated branch and then merge it back with a metadata-rich commit.

### Agent configuration

The plan commands (`vizier draft`, `vizier approve`, `vizier review`, `vizier merge`) use the scoped agent config described in the README. Declare defaults under `[agents.default]` and override the workflow-specific scopes to mix editing backends and the wire transport as needed:

```toml
[agents.default]
backend = "agent-alpha"

[agents.approve]
backend = "agent-alpha"      # enforce an editing-capable implementation backend

[agents.review]
backend = "agent-alpha"
[agents.review.backend]
profile = "compliance"

[agents.merge]
backend = "wire"             # keep merge cleanup on the wire stack
```

If the selected backend crashes or rejects the request, the command fails immediately with the backend error. Vizier no longer falls back to wire automatically; rerun the command once the configured backend is healthy.

CLI overrides (`--backend`, `--agent-bin`, `--agent-profile`, `--agent-bounds`, `-p/--model`, `-r/--reasoning-effort`) apply only to the command being executed and sit above the `[agents.<scope>]` entries. Vizier warns when a model override is ignored because the selected backend cannot honor it, so operators know when to adjust the per-command config instead of expecting wire-only flags to work everywhere.

## High-Level Timeline

1. **`vizier draft <spec>`** — Creates a `draft/<slug>` branch and writes `.vizier/implementation-plans/<slug>.md` inside a disposable worktree based on the primary branch. Your checkout stays untouched.
2. **`vizier approve <slug>`** — Applies the plan on `draft/<slug>` from within another temporary worktree, staging and committing the resulting edits on that branch only.
3. **`vizier review <slug>`** — Runs the configured review checks (defaults to `cargo check --all --all-targets` + `cargo test --all --all-targets` when a `Cargo.toml` exists), captures the diff summary, streams the configured backend’s critique to the terminal (and session log) instead of writing `.vizier/reviews/<slug>.md`, updates the plan status (e.g., `review-ready`), and optionally applies fixes on the plan branch.
4. **`vizier merge <slug>`** — Refreshes the plan branch, removes the plan document, and (by default) squashes the implementation edits into a single code commit on the target branch before writing the non–fast-forward merge commit that embeds the stored plan under an `Implementation Plan:` block. Pass `--no-squash` or set `[merge] squash = false` in `.vizier/config.toml` to keep the legacy “merge straight from the draft branch history” behavior.

Every step commits code and canonical narrative edits together in a single commit (`.vizier/.snapshot` plus root-level TODO threads). Plan documents under `.vizier/implementation-plans/`, `.vizier/tmp/*`, and session logs remain scratch artifacts and are filtered out of staging automatically.

At every stage you can pause, review the artifacts, and hand control back to a human maintainer.

Need to see what’s pending before approving or merging? Run `vizier list [--target BRANCH]` at any time to print every `draft/<slug>` branch that is ahead of the chosen target branch (defaults to the detected primary), along with the stored metadata summary.

> 💡 Quality-of-life: `vizier completions <bash|zsh|fish|powershell|elvish>` prints a dynamic completion script. Source it once (for example, `echo "source <(vizier completions zsh)" >> ~/.zshrc`) so Tab completion offers pending plan slugs whenever you run `vizier approve` or `vizier merge`.

### Holding commits with `--no-commit`

All plan workflow commands except `vizier merge` honor the global `--no-commit` flag (or `[workflow] no_commit_default = true` in `.vizier/config.toml`). When active, Vizier still runs the configured backend and writes artifacts, but it leaves the plan worktree dirty instead of committing or pushing:

- `vizier draft --no-commit` leaves the generated plan Markdown uncommitted under `.vizier/tmp-worktrees/.../`.
- `vizier approve --no-commit` applies the plan but preserves the worktree so you can diff and hand-edit before committing.
- `vizier review --no-commit` streams the critique to your terminal/session log and (optionally) applies backend-generated fixes without committing the plan document updates or any code changes.

Use this when you want to inspect agent output locally before history changes. Once satisfied, either commit inside the preserved worktree or rerun the same command without `--no-commit`. `vizier merge` still requires an actual merge commit, so finalize the draft branch (either manually or by rerunning `approve`/`review` without the flag) before merging.

### Customizing the plan/review/merge prompts

Repositories can tune every agent instruction involved in this workflow without recompiling Vizier. Define `[agents.<scope>.prompts.<kind>]` tables (for example, `[agents.draft.prompts.implementation_plan]`, `[agents.review.prompts.review]`, `[agents.merge.prompts.merge_conflict]`) inside `.vizier/config.toml` to point at custom Markdown templates via `path` or inline text and to pin backend/model/reasoning overrides for that specific scope. Vizier loads those profiles before each run, so prompt updates take effect immediately; legacy `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`, `.vizier/REVIEW_PROMPT.md`, and `.vizier/MERGE_CONFLICT_PROMPT.md` files remain as fallbacks when no profile is defined.

## `vizier draft`: create the plan branch

**Prerequisites**
- An editing-capable backend is selected for the `draft` scope.
- Primary branch is up to date (auto-detected via `origin/HEAD`, `main`, or `master`).

**What it does**
- Derives a slug from your spec (or `--name`) and creates `draft/<slug>` from the detected primary branch.
- Spawns a disposable worktree under `.vizier/tmp-worktrees/<slug>-<suffix>/` and runs the configured plan backend there so your current working tree never gets touched.
- Writes the backend-produced Markdown to `.vizier/implementation-plans/<slug>.md` and commits it on `draft/<slug>` with message `docs: add implementation plan <slug>`.
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
- An editing-capable backend selected for the `approve` scope.
- Plan branch (`draft/<slug>` or `--branch`) and target branch (`--target`, otherwise detected primary) exist locally.

**What it does**
- Validates that `draft/<slug>` is based on the current target branch; warns if the branch is behind.
- Creates a temporary worktree `.vizier/tmp-worktrees/<slug>-<suffix>/` checked out to the plan branch and runs the configured implementation backend against the stored plan document.
- The backend edits `.vizier/.snapshot`, TODOs, and code directly inside that worktree; Vizier stages `.` and commits the changes on the plan branch with the Auditor-provided commit message.
- Your original checkout stays untouched. On success the temp worktree is removed; on failure it is preserved for debugging and the branch keeps whatever the backend staged.
- While the backend runs, Vizier prints one `[agent:<scope>] phase — message` line per event (with status, percentage, and file hints) so you keep a scrolling history of what the agent is doing instead of watching a spinner. Pass `-q` to suppress them or `-v/-vv` for timestamps/raw JSON.

**Flags to remember**
- `vizier approve <slug>` — default flow.
- `vizier list [--target BRANCH]` — standalone command to print every `draft/*` branch ahead of the target before approving or merging.
- `vizier approve --target release/1.0` — preview and diff against a branch other than the detected primary.
- `vizier approve --branch feature/foo` — when your work diverges from `draft/<slug>` naming.
- `vizier approve -y` — skip the confirmation prompt.

**Git effects**
- Only `draft/<slug>` receives commits. The target branch and your working tree never change.
- Vizier prints `review with "git diff <target>...<draft/<slug>>"` so you can inspect the diff immediately.
- If the backend makes no changes, the command aborts with `agent completed without modifying files` to avoid empty commits.

## `vizier review`: critique the plan branch

**Prerequisites**
- Clean working tree (same guardrail as `approve`/`merge`)
- An editing-capable backend selected for the `review` scope
- Plan branch (`draft/<slug>` or `--branch`) is up to date enough that you can run the configured checks locally

**What it does**
- Creates another disposable worktree on `draft/<slug>`, gathers the diff against the target branch, and runs the configured review checks (defaults to `cargo check --all --all-targets` and `cargo test --all --all-targets` when a `Cargo.toml` is present or the `[review.checks]` commands in your config).
- Streams each check result to stderr so you see passes/failures before the backend speaks. Failures are captured verbatim and wired into the prompt.
- Builds an agent prompt that includes the snapshot, TODO threads, plan document, diff summary, and the check logs, then prints the Markdown critique directly to stdout (and into the session log) instead of saving `.vizier/reviews/<slug>.md`.
- Updates `.vizier/implementation-plans/<slug>.md` to `status: review-ready`, stages the plan refresh, and commits on the plan branch so reviewers have an auditable artifact without leaving temporary review files behind.
- Prompts `Apply suggested fixes on draft/<slug>? [y/N]` unless you passed `--review-only` or `-y/--yes`. When accepted, Vizier feeds the backend both the plan document and the in-memory critique text, applies the fixes on `draft/<slug>`, and stages/commits the result with a `review-addressed` status.

**Flags to remember**
- `vizier review <slug>` — default flow
- `--review-only` — skip the fix-up prompt; only emit the critique/check results
- `--skip-checks` — jump straight to the critique when your test suite is too heavy for disposable worktrees
- `-y/--yes` — apply fixes automatically after generating the critique
- `--target` / `--branch` — override the diff base or plan branch name when needed

**Checks & configuration**
- By default, Vizier tries `cargo check --all --all-targets` and `cargo test --all --all-targets` when `Cargo.toml` exists. Override this via:
  ```toml
  [review.checks]
  commands = ["npm test", "cargo fmt -- --check", "cargo clippy -- -D warnings"]
  ```
- Failed commands do not abort the review; the stderr/stdout are preserved and surfaced in both the CLI output and agent prompt so reviewers can see exactly what broke.
- Use `--skip-checks` when your repo relies on external services or long-running suites. The critique still includes the diff summary, plan metadata, and TODO threads.

**Outputs to watch**
- CLI prints `critique=terminal`, the diff command, check counts, and the session log path in the Outcome line; the critique itself appears earlier in stdout for immediate consumption.
- `draft/<slug>` only gains the updated plan document (status + timestamps); there is no `.vizier/reviews` directory.
- `git log draft/<slug>` shows a narrative commit for the critique and (optionally) a code commit for the auto-applied fixes.

## `vizier merge`: land the plan with metadata

**Prerequisites**
- Clean working tree.
- Plan branch contains agent-authored commits you want to land.
- You’re ready to remove `.vizier/implementation-plans/<slug>.md` from the branch.

**Prep work (handled automatically)**
1. Creates a temporary worktree on the plan branch.
2. Deletes `.vizier/implementation-plans/<slug>.md` from that branch so the plan doc does not land in the target branch.
3. Runs a `vizier save`–style backend refresh to make sure `.vizier/.snapshot` + TODOs in the plan branch reflect the latest story before merging.
4. Cleans up the worktree.
5. Checks out the target branch locally (switches branches if needed).

**Merge mechanics**
- Applies the combined plan diff onto the target branch as a single-parent implementation commit (squash mode) so every plan lands as “implementation commit + merge commit.” Use `--no-squash` when you explicitly want the target branch to inherit the draft branch history instead.
- Builds a merge commit with subject `feat: merge plan <slug>` and a body that only contains
  - An optional `Operator Note: …` line when `--note` is present.
  - An `Implementation Plan:` block that inlines the stored plan document (or a placeholder if the file cannot be read).
- Calls `vcs::prepare_merge` for a non–fast-forward merge. If there are no conflicts:
  - In squash mode, Vizier writes the implementation commit described above, runs any configured CI/CD gate, then commits the merge with parents `[implementation, draft/<slug>]`.
  - In `--no-squash` mode, Vizier immediately commits the merge with parents `[target, draft/<slug>]` just like Git’s default flow.
- When conflicts occur, Vizier writes `.vizier/tmp/merge-conflicts/<slug>.json` with the HEAD/source commit IDs and conflict list, then:
  - With `--auto-resolve-conflicts`, runs the configured backend inside the repo to try resolving and, if successful, finalizes the merge automatically.
  - Otherwise, instructs you to resolve conflicts manually, stage the files, and rerun `vizier merge <slug> --complete-conflict`; Vizier will detect the sentinel JSON and finish the merge once the index is clean, failing fast if no pending merge exists. In squash mode Vizier turns the resolved index into the implementation commit first, re-runs the CI gate, and only then writes the final merge commit so the “exactly two commits” contract still holds.
- Successful merges delete `draft/<slug>` automatically as long as the merge commit contains the branch tip; pass `--keep-branch` to retain the branch locally (legacy `--delete-branch` remains as a compatibility alias but is no longer required).
- `--yes` skips the confirmation prompt, `--complete-conflict` finalizes *only* an existing Vizier-managed merge (and errors when no sentinel is present), and `--target/--branch` behave like they do for `approve`.
- **CI/CD gate:** When `[merge.cicd_gate]` configures a script, Vizier executes it from the repo root while the implementation commit is staged but before the merge commit is written (squash mode) or immediately after the merge commit (legacy). A zero exit code finalizes the merge; a non-zero exit surfaces the script’s stdout/stderr and aborts so you can investigate (the implementation commit and draft branch are left intact). Set `auto_resolve = true` plus `retries = <n>` to let the backend attempt fixes when the gate fails. In squash mode Vizier amends the implementation commit when the backend applies fixes so the target branch still sees exactly two commits. Override the behavior per run with `--cicd-script PATH`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, and `--cicd-retries N`. Gate checks also run when resuming merges via `--complete-conflict`, so even manual conflict resolutions must pass the script before landing.

> **Manual completion tip:** After you resolve conflicts yourself, make sure you are checked out to the recorded target branch, stage the fixes, and then run `vizier merge <slug> --complete-conflict`. The flag refuses to run if Git is not in the middle of the stored merge or if no sentinel JSON exists, which protects history from accidental merges.

**Post-merge artifacts**
- Merge commit on the target branch titled `feat: merge plan <slug>` with the plan document embedded under `Implementation Plan:` (plus any optional operator note).
- Optional branch deletion (local only).
- `.vizier/tmp/merge-conflicts/<slug>.json` cleaned up automatically when the merge completes.

## Failure & recovery playbook

| Situation | Recovery |
| --- | --- |
| Draft worktree creation fails | Vizier deletes the stub branch unless the plan file was already committed. Re-run `vizier draft` once the error is fixed. |
| `vizier approve` fails mid-run | Temp worktree path is printed; inspect it to salvage partially staged files, then rerun once corrected. The plan branch remains intact. |
| Merge conflicts | Resolve conflicts on the target branch, stage the files, rerun `vizier merge <slug> --complete-conflict`. Vizier reuses `.vizier/tmp/merge-conflicts/<slug>.json` to finalize and fails fast if no pending merge exists. |
| Need to resume after aborting Git’s merge | As long as the sentinel JSON still exists and `git status` is clean, rerunning `vizier merge <slug>` finishes the in-progress merge without repeating refresh/removal steps. |
| CI/CD gate failure | The merge commit and draft branch remain untouched. Inspect the printed script output, apply fixes manually or rerun with `--auto-cicd-fix` (requires a backend that supports auto-fixes), and retry `vizier merge <slug>` once the gate script exits 0. |
| Backend auto-resolution fails | Vizier warns and falls back to manual instructions. Resolve/stage/retry just like a normal Git merge. |

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
   - Hit a conflict? Resolve it on `main`, stage the changes, and rerun `vizier merge ingest-backpressure --complete-conflict` to finish the stored merge without creating ad-hoc commits.

Throughout the process, Outcome lines and Auditor records cite the plan slug, affected files, and any pending commit gates so auditors can trace who approved what. Tie this workflow into the broader agent-orchestration story by referencing this document in `AGENTS.md` or external SOPs when onboarding third-party agents.

## FAQ

**Can I run `vizier approve` without re-drafting?**  
Yes. If you manually edit `.vizier/implementation-plans/<slug>.md` on the draft branch, rerun `vizier approve <slug>` to reapply the plan. Vizier refuses to commit if the backend makes no changes so you can iterate safely.

**What if the draft branch lags far behind the target branch?**  
`vizier approve` prints a warning when `draft/<slug>` is missing commits from the target. You can merge or rebase the draft branch manually before running `vizier merge`, or accept that `vizier merge` may surface conflicts which you’ll resolve with the sentinel + `--complete-conflict` workflow described above.

**Does `vizier merge` push to origin?**  
Not automatically. Pass `--push` (global CLI flag) after `approve` or `merge` if you want Vizier to push once the command succeeds, or push manually when you’re ready.

## Architecture docs & agent workflows

- Plan documents live under `.vizier/implementation-plans/` and describe how the selected backend intends to implement the spec, but they do **not** replace architecture-doc references required by the compliance gate. Keep your architecture doc path handy so `vizier save` can cite it when the plan’s changes land on the primary branch.
- When multiple agents collaborate, use this workflow as the runbook: one operator drafts, another approves, and a third merges. The Auditor already records Outcome lines for each command, so reference those facts (plus the plan slug) inside `.vizier/.snapshot` and TODO updates to keep the narrative threads aligned.
