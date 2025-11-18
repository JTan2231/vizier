# Draft → Approve → Review → Merge Workflow

This guide explains how Vizier’s plan workflow turns a high-level spec into audited code without disturbing your working tree. Use it whenever you want Vizier (or an external agent) to implement a scoped change on a dedicated branch and then merge it back with a metadata-rich commit.

### Agent configuration

The plan commands (`vizier draft`, `vizier approve`, `vizier review`, `vizier merge`) use the scoped agent config described in the README. Declare defaults under `[agents.default]` and override the workflow-specific scopes to mix Codex and wire backends as needed:

```toml
[agents.default]
backend = "codex"
fallback_backend = "wire"

[agents.approve]
backend = "codex"      # enforce Codex-backed implementation

[agents.review]
backend = "codex"
[agents.review.codex]
profile = "compliance"

[agents.merge]
backend = "wire"       # keep merge cleanup on the wire stack
```

CLI overrides (`--backend`, `--codex-*`, `-p/--model`, `-r/--reasoning-effort`) apply only to the command being executed and sit above the `[agents.<scope>]` entries. Vizier warns when a model override is ignored because Codex is active, so operators know when to adjust the per-command config instead of expecting wire-only flags to work everywhere.

## High-Level Timeline

1. **`vizier draft <spec>`** — Creates a `draft/<slug>` branch and writes `.vizier/implementation-plans/<slug>.md` inside a disposable worktree based on the primary branch. Your checkout stays untouched.
2. **`vizier approve <slug>`** — Applies the plan on `draft/<slug>` from within another temporary worktree, staging and committing the resulting edits on that branch only.
3. **`vizier review <slug>`** — Runs the configured review checks (defaults to `cargo check --all --all-targets` + `cargo test --all --all-targets` when a `Cargo.toml` exists), captures the diff summary, generates a Codex critique at `.vizier/reviews/<slug>.md`, updates the plan status (e.g., `review-ready`), and optionally applies fixes on the plan branch.
4. **`vizier merge <slug>`** — Refreshes the plan branch, removes the plan document, and performs a non–fast-forward merge into the target branch with the stored plan embedded under an `Implementation Plan:` block in the merge commit.

At every stage you can pause, review the artifacts, and hand control back to a human maintainer.

Need to see what’s pending before approving or merging? Run `vizier list [--target BRANCH]` at any time to print every `draft/<slug>` branch that is ahead of the chosen target branch (defaults to the detected primary), along with the stored metadata summary.

> 💡 Quality-of-life: `vizier completions <bash|zsh|fish|powershell|elvish>` prints a dynamic completion script. Source it once (for example, `echo "source <(vizier completions zsh)" >> ~/.zshrc`) so Tab completion offers pending plan slugs whenever you run `vizier approve` or `vizier merge`.

### Holding commits with `--no-commit`

All plan workflow commands except `vizier merge` honor the global `--no-commit` flag (or `[workflow] no_commit_default = true` in `.vizier/config.toml`). When active, Vizier still runs Codex and writes artifacts, but it leaves the plan worktree dirty instead of committing or pushing:

- `vizier draft --no-commit` leaves the generated plan Markdown uncommitted under `.vizier/tmp-worktrees/.../`.
- `vizier approve --no-commit` applies the plan but preserves the worktree so you can diff and hand-edit before committing.
- `vizier review --no-commit` records the critique and (optionally) Codex fixes without committing the review file or fixes.

Use this when you want to inspect Codex output locally before history changes. Once satisfied, either commit inside the preserved worktree or rerun the same command without `--no-commit`. `vizier merge` still requires an actual merge commit, so finalize the draft branch (either manually or by rerunning `approve`/`review` without the flag) before merging.

### Customizing the plan/review/merge prompts

Repositories can tune every Codex instruction involved in this workflow without recompiling Vizier. Drop Markdown files under `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`, `.vizier/REVIEW_PROMPT.md`, and `.vizier/MERGE_CONFLICT_PROMPT.md` (or set `[prompts.implementation_plan|review|merge_conflict]` in your config) to change how draft plans are generated, how reviews critique a branch, and how Codex handles merge conflicts. The CLI reads these templates when it starts, so restart `vizier` after editing them to ensure new instructions take effect.

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
- While Codex runs, Vizier prints one `[codex] phase — message` line per Codex event (with status, percentage, and file hints) so you keep a scrolling history of what the agent is doing instead of watching a spinner. Pass `-q` to suppress them or `-v/-vv` for timestamps/raw JSON.

**Flags to remember**
- `vizier approve <slug>` — default flow.
- `vizier list [--target BRANCH]` — standalone command to print every `draft/*` branch ahead of the target before approving or merging.
- `vizier approve --target release/1.0` — preview and diff against a branch other than the detected primary.
- `vizier approve --branch feature/foo` — when your work diverges from `draft/<slug>` naming.
- `vizier approve -y` — skip the confirmation prompt.

**Git effects**
- Only `draft/<slug>` receives commits. The target branch and your working tree never change.
- Vizier prints `review with "git diff <target>...<draft/<slug>>"` so you can inspect the diff immediately.
- If Codex makes no changes, the command aborts with `Codex completed without modifying files` to avoid empty commits.

## `vizier review`: critique the plan branch

**Prerequisites**
- Clean working tree (same guardrail as `approve`/`merge`)
- Codex backend enabled
- Plan branch (`draft/<slug>` or `--branch`) is up to date enough that you can run the configured checks locally

**What it does**
- Creates another disposable worktree on `draft/<slug>`, gathers the diff against the target branch, and runs the configured review checks (defaults to `cargo check --all --all-targets` and `cargo test --all --all-targets` when a `Cargo.toml` is present or the `[review.checks]` commands in your config).
- Streams each check result to stderr so you see passes/failures before Codex speaks. Failures are captured verbatim and wired into the prompt.
- Builds a Codex prompt that includes the snapshot, TODO threads, plan document, diff summary, and the check logs, then writes Codex’s Markdown critique to `.vizier/reviews/<slug>.md` with front-matter `{plan, branch, target, reviewed_at, reviewer}`.
- Updates `.vizier/implementation-plans/<slug>.md` to `status: review-ready`, stages the review + plan refresh, and commits on the plan branch so reviewers have an auditable artifact.
- Prompts `Apply suggested fixes on draft/<slug>? [y/N]` unless you passed `--review-only` or `-y/--yes`. When accepted, Vizier feeds Codex both the plan document and the saved review file, applies the fixes on `draft/<slug>`, and stages/commits the result with a `review-addressed` status.

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
- Failed commands do not abort the review; the stderr/stdout are preserved and surfaced in both the CLI output and Codex prompt so reviewers can see exactly what broke.
- Use `--skip-checks` when your repo relies on external services or long-running suites. The critique still includes the diff summary, plan metadata, and TODO threads.

**Outputs to watch**
- CLI prints the review file path, diff command, check counts, and session log path in the Outcome line.
- `draft/<slug>` gains `.vizier/reviews/<slug>.md` plus the updated plan document (status + timestamps).
- `git log draft/<slug>` shows a narrative commit for the critique and (optionally) a code commit for the auto-applied fixes.

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
  - Otherwise, instructs you to resolve conflicts manually, stage the files, and rerun `vizier merge <slug> --complete-conflict`; Vizier will detect the sentinel JSON and finish the merge once the index is clean, failing fast if no pending merge exists.
- Successful merges delete `draft/<slug>` automatically as long as the merge commit contains the branch tip; pass `--keep-branch` to retain the branch locally (legacy `--delete-branch` remains as a compatibility alias but is no longer required).
- `--yes` skips the confirmation prompt, `--complete-conflict` finalizes *only* an existing Vizier-managed merge (and errors when no sentinel is present), and `--target/--branch` behave like they do for `approve`.
- **CI/CD gate:** When `[merge.cicd_gate]` configures a script, Vizier executes it from the repo root after staging the merge commit but before deleting the draft branch or pushing. A zero exit code finalizes the merge; a non-zero exit surfaces the script’s stdout/stderr and aborts so you can investigate (the merge commit and draft branch are left intact). Set `auto_resolve = true` plus `retries = <n>` to let Codex attempt fixes when the gate fails, and override the behavior per run with `--cicd-script PATH`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, and `--cicd-retries N`. Gate checks also run when resuming merges via `--complete-conflict`, so even manual conflict resolutions must pass the script before landing.

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
| CI/CD gate failure | The merge commit and draft branch remain untouched. Inspect the printed script output, apply fixes manually or rerun with `--auto-cicd-fix` (Codex backend required), and retry `vizier merge <slug>` once the gate script exits 0. |
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
   - Hit a conflict? Resolve it on `main`, stage the changes, and rerun `vizier merge ingest-backpressure --complete-conflict` to finish the stored merge without creating ad-hoc commits.

Throughout the process, Outcome lines and Auditor records cite the plan slug, affected files, and any pending commit gates so auditors can trace who approved what. Tie this workflow into the broader agent-orchestration story by referencing this document in `AGENTS.md` or external SOPs when onboarding third-party agents.

## FAQ

**Can I run `vizier approve` without re-drafting?**  
Yes. If you manually edit `.vizier/implementation-plans/<slug>.md` on the draft branch, rerun `vizier approve <slug>` to reapply the plan. Vizier refuses to commit if Codex makes no changes so you can iterate safely.

**What if the draft branch lags far behind the target branch?**  
`vizier approve` prints a warning when `draft/<slug>` is missing commits from the target. You can merge or rebase the draft branch manually before running `vizier merge`, or accept that `vizier merge` may surface conflicts which you’ll resolve with the sentinel + `--complete-conflict` workflow described above.

**Does `vizier merge` push to origin?**  
Not automatically. Pass `--push` (global CLI flag) after `approve` or `merge` if you want Vizier to push once the command succeeds, or push manually when you’re ready.

## Architecture docs & agent workflows

- Plan documents live under `.vizier/implementation-plans/` and describe how Codex intends to implement the spec, but they do **not** replace architecture-doc references required by the compliance gate. Keep your architecture doc path handy so `vizier save` can cite it when the plan’s changes land on the primary branch.
- When multiple agents collaborate, use this workflow as the runbook: one operator drafts, another approves, and a third merges. The Auditor already records Outcome lines for each command, so reference those facts (plus the plan slug) inside `.vizier/.snapshot` and TODO updates to keep the narrative threads aligned.
