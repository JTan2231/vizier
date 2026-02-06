# Draft → Approve → Review → Merge Workflow

This guide explains how Vizier’s plan workflow turns a high-level spec into audited code without disturbing your working tree. Use it whenever you want Vizier (or an external agent) to implement a scoped change on a dedicated branch and then merge it back with a metadata-rich commit.

## Queue Plan Pipelines with `vizier build`

When you want to batch related plan drafts, start with:

```bash
vizier build --file examples/build/todo.toml --name todo-batch
```

Create mode writes a build session to `build/<id>` with plan artifacts under `.vizier/implementation-plans/builds/<id>/`. To turn that session into executable plan work, run:

```bash
vizier build execute todo-batch --yes
```

Execution mode queues per-step scheduler jobs that materialize canonical `draft/<slug>` plan docs, then runs `approve`/`review`/`merge` phases according to resolved build policy. By default, pipeline/target/review behavior comes from `[build]` config and optional step-level overrides in the build file; pass `--pipeline ...` only when you need a one-off global override. Use `--resume` to continue from `execution.json` without duplicating queued/running jobs; resume rejects policy drift if the resolved per-step policy changed.

For the build schema, execute options, and artifact details, see `docs/user/build.md`, `examples/build/todo.toml`, and `examples/build/todo.json`.

### Agent configuration

The plan commands (`vizier draft`, `vizier approve`, `vizier review`, `vizier merge`) use the scoped agent config described in the README. Declare defaults under `[agents.default]` and override the workflow-specific scopes to mix editing stacks as needed (see also `docs/user/prompt-config-matrix.md` for the full scope×prompt-kind matrix and config levers):

```toml
[agents.default]
agent = "codex"

[agents.approve]
agent = "codex"            # enforce an editing-capable implementation backend

[agents.review]
agent = "codex"

[agents.merge]
agent = "gemini"           # keep merge cleanup on the gemini stack
```

For the full catalogue of knobs and defaults (scope, precedence, CLI overrides), see `docs/user/config-reference.md`. Before kicking off a workflow, `vizier plan --json` prints the resolved agent/prompt settings for your current repo without mutating state.

Config precedence: when you skip `--config-file`, Vizier loads the user/global config from `$XDG_CONFIG_HOME`/`$VIZIER_CONFIG_DIR` (if present) as a base and overlays `.vizier/config.toml` so repo settings override while missing keys inherit your defaults. `VIZIER_CONFIG_FILE` is only consulted when neither config file exists.

If the selected agent crashes or rejects the request, the command fails immediately with the shim error. Vizier does not fall back to another agent; rerun the command once the configured selector is healthy.

CLI overrides (`--agent`, `--agent-label`, `--agent-command`) apply only to the command being executed and sit above the `[agents.<scope>]` entries.

Need to sanity-check how those layers resolve before you kick off the workflow? Run `vizier plan` (or `vizier plan --json` for structured output) to print the effective configuration, per-scope agent selector, and resolved runtime without mutating the repo or starting a session.

Agent runs require either a bundled selector (for example, `agent = "codex"`/`"gemini"`) or an explicit script path via `[agents.<scope>.agent].command = ["/path/to/script.sh"]`. Bundled shims install under `share/vizier/agents/` and follow the runner contract: stdout is the assistant text, stderr is progress/errors, and the exit code sets status. There is no autodiscovery fallback when no script is provided.

Want to exercise a scoped agent without touching `.vizier` or Git? `vizier test-display [--scope ask|save|draft|approve|review|merge]` streams progress through the normal display stack using a harmless prompt, reports the agent’s exit code/duration/stdout/stderr, and defaults to no session logging (`--session` opts back in).

## High-Level Timeline

1. **`vizier draft <spec>`** — Creates a `draft/<slug>` branch and writes `.vizier/implementation-plans/<slug>.md` inside a disposable worktree based on the primary branch. Your checkout stays untouched.
2. **Manual plan edits (optional)** — If the plan needs clarification, edit `.vizier/implementation-plans/<slug>.md` directly on the `draft/<slug>` branch and commit the update.
3. **`vizier approve <slug>`** — Applies the plan on `draft/<slug>` from within another temporary worktree, staging and committing the resulting edits on that branch only.
4. **`vizier review <slug>`** — Runs the merge CI/CD gate before prompting (using `[merge.cicd_gate]` or review `--cicd-*` overrides) and feeds the result into the critique prompt/summary/session log, then runs the configured review checks (defaults to `cargo check --all --all-targets` + `cargo test --all --all-targets` when a `Cargo.toml` exists). The critique streams to the terminal (and session log) instead of writing `.vizier/reviews/<slug>.md`, and you can optionally apply fixes on the plan branch without mutating the plan document’s front matter. Auto-remediation for the gate stays disabled during review; merge still owns CI/CD fixes.
5. **`vizier merge <slug>`** — Refreshes the plan branch, removes the plan document, replays the plan branch commits onto the target, and (by default) soft-squashes that range into a single implementation commit on the target before writing the non–fast-forward merge commit that embeds the stored plan under an `Implementation Plan:` block. Pass `--no-squash` or set `[merge] squash = false` in `.vizier/config.toml` to keep the legacy “merge straight from the draft branch history” behavior. If the plan branch contains merge commits, squash merges now preflight the history and require either `--squash-mainline <parent index>` (or `[merge] squash_mainline = <n>`) to cherry-pick those merges or `--no-squash` to keep the branch graph intact.

Every step commits code and canonical narrative edits together in a single commit; prompts remind you to update `.vizier/narrative/snapshot.md` alongside `.vizier/narrative/glossary.md` (plus notes under `.vizier/narrative/threads/`), but that pairing is not enforced by the CLI. Plan documents under `.vizier/implementation-plans/`, `.vizier/tmp/*`, and session logs remain scratch artifacts and are filtered out of staging automatically.

At every stage you can pause, review the artifacts, and hand control back to a human maintainer.

All assistant-backed commands now enqueue background jobs. Use `--follow` when you want to stream the job’s stdout/stderr to your terminal; otherwise inspect progress with `vizier jobs`. Scheduled commands do not support `--json` output; use `vizier jobs show --format json` instead. On a TTY, `vizier approve`/`vizier merge` prompt for confirmation before the job is queued, and `vizier review` prompts for the review mode unless you pass `--yes`/`--review-only`/`--review-file`; non-TTY runs require explicit flags. For explicit ordering across unrelated jobs, pass repeatable `--after <job-id>` flags when queueing any scheduler-backed command.

Need to see what’s pending before approving or merging? Run `vizier list [--target BRANCH]` at any time to print every `draft/<slug>` branch that is ahead of the chosen target branch (defaults to the detected primary). By default each entry renders as a label/value block with `Plan`, `Branch`, and `Summary`, and when a matching background job exists the block adds inline job details (`Job`, `Job status`, optional `Job scope`, optional `Job started`) plus copy-pastable commands (`Status`, `Logs` with `--follow`, `Attach`). You can switch to table/JSON output or trim the fields via `[display.lists.list]` config or `vizier list --format/--fields`. The empty state returns a single `Outcome: No pending draft branches` block.

Jobs can sit in `waiting_on_deps` or `waiting_on_locks` when upstream artifacts, explicit `--after` predecessors, or locks are not yet available. Use `vizier jobs show <id>` to see `After`, dependency, lock, pinned-head, and wait-reason details when a job is queued, or add `After`, `Wait`, `Dependencies`, `Locks`, and `Pinned head` to `[display.lists.jobs].fields` so `vizier jobs list` surfaces them inline. If a segment ends in `failed`/`blocked_by_dependency` and you want to re-run from that point, use `vizier jobs retry <job-id>` to rewind the job plus downstream dependents and let the scheduler start them again.

> 💡 Quality-of-life: `vizier completions <bash|zsh|fish|powershell|elvish>` prints a dynamic completion script. Source it once (for example, `echo "source <(vizier completions zsh)" >> ~/.zshrc`) so Tab completion offers pending plan slugs whenever you run `vizier approve` or `vizier merge`.

### Holding commits with `--no-commit`

All plan workflow commands except `vizier merge` honor the global `--no-commit` flag (or `[workflow] no_commit_default = true` in `.vizier/config.toml`). When active, Vizier still runs the configured agent and writes artifacts, but it leaves the plan worktree dirty instead of committing or pushing:

- `vizier draft --no-commit` leaves the generated plan Markdown uncommitted under `.vizier/tmp-worktrees/.../`.
- `vizier approve --no-commit` applies the plan but preserves the worktree so you can diff and hand-edit before committing.
- `vizier review --no-commit` streams the critique to your terminal/session log and (optionally) applies agent-generated fixes without committing the plan document updates or any code changes.

Use this when you want to inspect agent output locally before history changes. Once satisfied, either commit inside the preserved worktree or rerun the same command without `--no-commit`. `vizier merge` still requires an actual merge commit, so finalize the draft branch (either manually or by rerunning `approve`/`review` without the flag) before merging.

### Customizing the plan/review/merge prompts

Repositories can tune every agent instruction involved in this workflow without recompiling Vizier. Define `[agents.<scope>.prompts.<kind>]` tables (for example, `[agents.draft.prompts.implementation_plan]`, `[agents.review.prompts.review]`, `[agents.merge.prompts.merge_conflict]`) inside `.vizier/config.toml` to point at custom Markdown templates via `path` or inline text and to pin per-prompt agent/runtime/documentation overrides for that specific scope. Vizier loads those profiles before each run, so prompt updates take effect immediately; `.vizier/DOCUMENTATION_PROMPT.md`, `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`, `.vizier/REVIEW_PROMPT.md`, and `.vizier/MERGE_CONFLICT_PROMPT.md` remain as fallbacks when no profile is defined. Per-scope documentation toggles live under `[agents.<scope>.documentation]` (`enabled`, `include_snapshot`, `include_narrative_docs`) so scopes like merge/approve/review-fix can opt out of the documentation prompt or drop snapshot/narrative attachments when they need a leaner context. For a complete matrix of scopes, prompt kinds, and fallback order, refer to `docs/user/prompt-config-matrix.md`.

Prompt text resolution is limited to `[agents.<scope>.prompts.<kind>]` -> `.vizier/*_PROMPT.md` -> baked-in defaults; `[prompts]` and `[prompts.<scope>]` tables are ignored, and `.vizier/BASE_SYSTEM_PROMPT.md` is not read. Prompt kinds are limited to `documentation`, `commit`, `implementation_plan`, `review`, and `merge_conflict` (aliases like `base`, `system`, `plan`, `refine`, `merge` are rejected). Documentation context comes only from `.vizier/narrative/` (snapshot, glossary, threads); `.vizier/todo_*.md` is not read.

## `vizier draft`: create the plan branch

**Prerequisites**
- An editing-capable agent is selected for the `draft` scope.
- Primary branch is up to date (auto-detected via `origin/HEAD`, `main`, or `master`).

**What it does**
- Derives a slug from your spec (or `--name`) and creates `draft/<slug>` from the detected primary branch.
- Spawns a disposable worktree under `.vizier/tmp-worktrees/<slug>-<suffix>/` and runs the configured plan agent there so your current working tree never gets touched.
- Writes the agent-produced Markdown to `.vizier/implementation-plans/<slug>.md` and commits it on `draft/<slug>` with message `docs: add implementation plan <slug>`.
- Removes the temporary worktree when successful, printing the on-disk plan so you can review it.

**Flags to remember**
- `vizier draft "spec"` — inline operator spec.
- `vizier draft --file SPEC.md` — load spec from disk.
- `vizier draft --name hotfix-foo "..."` — override the slug + branch name.
- `vizier draft ... --after <job-id> [--after <job-id> ...]` — require explicit predecessor jobs to succeed before this draft can start.

**How to verify**
```bash
git branch --list 'draft/*'
git log --oneline draft/<slug> | head -n 3
```
Both commands should show the plan commit sitting one commit ahead of the primary branch while your current working tree remains clean.

## `vizier approve`: implement the plan safely

**Prerequisites**
- Clean working tree (enforced by `vcs::ensure_clean_worktree`).
- An editing-capable agent selected for the `approve` scope.
- Plan branch (`draft/<slug>` or `--branch`) and target branch (`--target`, otherwise detected primary) exist locally.

**What it does**
- Validates that `draft/<slug>` is based on the current target branch; warns if the branch is behind.
- Creates a temporary worktree `.vizier/tmp-worktrees/<slug>-<suffix>/` checked out to the plan branch and runs the configured implementation agent against the stored plan document.
- The agent edits `.vizier/narrative/snapshot.md`, `.vizier/narrative/glossary.md`, narrative docs, and code directly inside that worktree; Vizier stages `.` and commits the changes on the plan branch with the Auditor-provided commit message.
- Your original checkout stays untouched. On success the temp worktree is removed; on failure it is preserved for debugging and the branch keeps whatever the agent staged.
- While the agent runs, Vizier prints one `[agent:<scope>] phase — message` line per event (with status, percentage, and file hints) so you get a scrolling history of what the agent is doing. Pass `-q` to suppress them or `-v/-vv` for timestamps/raw JSON.

**Flags to remember**
- `vizier approve <slug>` — default flow.
- `vizier list [--target BRANCH]` — standalone command to print every `draft/*` branch ahead of the target before approving or merging.
- `vizier approve --target release/1.0` — preview and diff against a branch other than the detected primary.
- `vizier approve --branch feature/foo` — when your work diverges from `draft/<slug>` naming.
- `vizier approve -y` — skip the confirmation prompt (non-TTY runs require this).
- `vizier approve <slug> --after <job-id>` — enforce explicit job-id ordering when artifact dependencies are not enough.

**Optional stop-condition**
- Configure `[approve.stop_condition]` in `.vizier/config.toml` (and optionally override per run with `vizier approve --stop-condition-script <PATH> --stop-condition-retries <COUNT>`) to gate approve on a repo-local shell script.
- When a stop-condition script is set, Vizier always runs the agent at least once, then executes `sh <script>` from the approve worktree. A zero exit code means “done”; any non-zero exit triggers another agent attempt while retries remain.
- `retries` counts extra attempts after the first agent run. With the default `retries = 3`, approve can invoke the agent up to four times before giving up. If the script never exits 0, `vizier approve` fails, preserves the worktree, and prints guidance pointing at the worktree path and script output.
- Stop-condition scripts are best used for lightweight local checks (idempotence, smoke tests) that can safely run multiple times against the evolving draft branch; they should tolerate being re-run and avoid destructive side effects.

**Git effects**
- Only `draft/<slug>` receives commits. The target branch and your working tree never change.
- Vizier prints `review with "git diff <target>...<draft/<slug>>"` so you can inspect the diff immediately.
- If the agent makes no changes, the command aborts with `agent completed without modifying files` to avoid empty commits.

## `vizier review`: critique the plan branch

**Prerequisites**
- Clean working tree (same guardrail as `approve`/`merge`)
- An editing-capable agent selected for the `review` scope
- Plan branch (`draft/<slug>` or `--branch`) is up to date enough that you can run the configured checks locally

**What it does**
- Runs the merge CI/CD gate before prompting, using `[merge.cicd_gate]` (or review `--cicd-*` overrides) and piping the status/log snippets into both the critique prompt and the review summary/session log. Auto-remediation for the gate is disabled during review; merge still owns CI/CD fixes.
- Creates another disposable worktree on `draft/<slug>`, gathers the diff against the target branch, and runs the configured review checks (defaults to `cargo check --all --all-targets` and `cargo test --all --all-targets` when a `Cargo.toml` is present or the `[review.checks]` commands in your config).
- Streams each check result to stderr so you see passes/failures before the agent speaks. Failures are captured verbatim and wired into the prompt.
- Builds an agent prompt that includes the snapshot, narrative docs, plan document, diff summary, and the check logs, then prints the Markdown critique directly to stdout (and into the session log) instead of saving `.vizier/reviews/<slug>.md`.
- On a TTY, prompts for the review mode before queueing unless you passed `--review-only`, `--review-file`, or `-y/--yes`; non-TTY runs require one of those flags. When apply-fixes is selected, Vizier feeds the agent both the plan document and the in-memory critique text, applies the fixes on `draft/<slug>`, and commits those changes (or leaves them pending with `--no-commit`). The plan document front matter stays lean (`plan` + `branch`) and is no longer mutated by review status updates.

**Flags to remember**
- `vizier review <slug>` — default flow
- `--review-only` — skip the fix-up prompt; only emit the critique/check results
- `--review-file` — write the critique to `vizier-review.md` in the repo root and skip fixes
- `--skip-checks` — jump straight to the critique when your test suite is too heavy for disposable worktrees
- `-y/--yes` — apply fixes automatically after generating the critique
- `--target` / `--branch` — override the diff base or plan branch name when needed
- `--cicd-script` / `--auto-cicd-fix` / `--no-auto-cicd-fix` / `--cicd-retries` — reuse the merge CI/CD gate script and overrides for review visibility; the gate still runs read-only during review (auto fixes remain merge-only).
- `--after <job-id>` (repeatable) — delay review until explicit predecessor jobs have succeeded.

**Checks & configuration**
- By default, Vizier tries `cargo check --all --all-targets` and `cargo test --all --all-targets` when `Cargo.toml` exists. Override this via:
  ```toml
  [review.checks]
  commands = ["npm test", "cargo fmt -- --check", "cargo clippy -- -D warnings"]
  ```
- Failed commands do not abort the review; the stderr/stdout are preserved and surfaced in both the CLI output and agent prompt so reviewers can see exactly what broke. The same applies to CI/CD gate failures: the critique still runs with the gate output attached.
- Use `--skip-checks` when your repo relies on external services or long-running suites. The critique still includes the diff summary, plan metadata, and narrative context.

**Outputs to watch**
- CLI prints `critique=terminal`, the diff command, check counts, and the session log path in the Outcome line; the critique itself appears earlier in stdout for immediate consumption.
- `draft/<slug>` gains the streamed critique and any optional fix commits; the plan document stays untouched and there is no `.vizier/reviews` directory.
- `git log draft/<slug>` shows a narrative commit for the critique and (optionally) a code commit for the auto-applied fixes.

## `vizier merge`: land the plan with metadata

**Prerequisites**
- Clean working tree.
- Plan branch contains agent-authored commits you want to land.
- You’re ready to remove `.vizier/implementation-plans/<slug>.md` from the branch.

**Prep work (handled automatically)**
1. Creates a temporary worktree on the plan branch.
2. Deletes `.vizier/implementation-plans/<slug>.md` from that branch so the plan doc does not land in the target branch.
3. Runs a `vizier save`–style agent refresh to make sure `.vizier/narrative/snapshot.md`, `.vizier/narrative/glossary.md`, and narrative docs in the plan branch reflect the latest story before merging.
4. Cleans up the worktree.
5. Checks out the target branch locally (switches branches if needed).

**Merge mechanics**
- Replays the plan branch commits onto the target head, then soft-resets back to that starting point to create a single-parent implementation commit (squash mode) so every plan lands as “implementation commit + merge commit.” Use `--no-squash` when you explicitly want the target branch to inherit the draft branch history instead.
- When the plan branch history includes merge commits, squash mode refuses to start until you choose a mainline parent: pass `--squash-mainline <parent index>` or set `[merge] squash_mainline = <n>` in `.vizier/config.toml`. For ambiguous histories (for example, octopus merges), Vizier aborts early with guidance to rerun `vizier merge` with `--no-squash` instead of failing mid-cherry-pick.
- Builds a merge commit with subject `feat: merge plan <slug>` and a body that only contains
  - An optional `Operator Note: …` line when `--note` is present.
  - An `Implementation Plan:` block that inlines the stored plan document (or a placeholder if the file cannot be read).
- In squash mode Vizier runs the cherry-pick + soft-squash flow, executes any configured CI/CD gate against the squashed implementation commit, then commits the merge with a single parent (the implementation commit) so the draft branch is no longer part of the target’s ancestry. In `--no-squash` mode it performs a normal non–fast-forward merge with parents `[target, draft/<slug>]`.
- When conflicts occur, Vizier writes `.vizier/tmp/merge-conflicts/<slug>.json` with the HEAD/source commit IDs (plus cherry-pick replay metadata in squash mode) and conflict list, then:
  - If conflict auto-resolution is enabled (default driven by `[merge.conflicts].auto_resolve` and overridable per-run with `--auto-resolve-conflicts` / `--no-auto-resolve-conflicts`), Vizier runs the configured agent inside the repo on both the initial attempt and any `--complete-conflict` resumes; success finishes the cherry-pick replay/implementation commit and completes the merge automatically. Agent-capable selectors are required for this path.
  - Otherwise, instructs you to resolve conflicts manually, stage the files, and rerun `vizier merge <slug> --complete-conflict`; Vizier will detect the sentinel JSON and finish the merge once the index is clean, failing fast if no pending merge exists. In squash mode Vizier finalizes the in-progress cherry-pick, replays any remaining plan commits, performs the soft squash, re-runs the CI gate, and only then writes the final merge commit so the “exactly two commits” contract still holds.
- Conflict auto-resolution is separate from `[merge.cicd_gate].auto_resolve`, which only controls CI/CD gate remediation when the script fails.
- Successful merges delete `draft/<slug>` automatically when the finalized merge references the recorded implementation commit; pass `--keep-branch` to retain the branch locally.
- Merge runs are non-interactive once queued; on a TTY you will be asked to confirm before the job is queued, otherwise pass `--yes`. `--complete-conflict` finalizes *only* an existing Vizier-managed merge (and errors when no sentinel is present), and `--target/--branch` behave like they do for `approve`.
- `--after <job-id>` is repeatable on merge as well, allowing explicit sequencing behind unrelated background jobs when branch/artifact dependencies alone are insufficient.
- **CI/CD gate:** When `[merge.cicd_gate]` configures a script, Vizier executes it from the repo root while the implementation commit is staged but before the merge commit is written (squash mode) or immediately after the merge commit (legacy). A zero exit code finalizes the merge; a non-zero exit surfaces the script’s stdout/stderr and aborts so you can investigate (the implementation commit and draft branch are left intact). Set `auto_resolve = true` plus `retries = <n>` to let the agent attempt fixes when the gate fails. In squash mode Vizier amends the implementation commit when the agent applies fixes so the target branch still sees exactly two commits. Override the behavior per run with `--cicd-script PATH`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, and `--cicd-retries N`. Gate checks also run when resuming merges via `--complete-conflict`, so even manual conflict resolutions must pass the script before landing.

> **Manual completion tip:** After you resolve conflicts yourself, make sure you are checked out to the recorded target branch, stage the fixes, and then run `vizier merge <slug> --complete-conflict`. The flag refuses to run if Git is not in the middle of the stored merge or if no sentinel JSON exists, which protects history from accidental merges.

**Post-merge artifacts**
- Merge commit on the target branch titled `feat: merge plan <slug>` with the plan document embedded under `Implementation Plan:` (plus any optional operator note).
- Optional branch deletion (local only).
- `.vizier/tmp/merge-conflicts/<slug>.json` cleaned up automatically when the merge completes.

## Failure & recovery playbook

| Situation | Recovery |
| --- | --- |
| Failed/blocked scheduler segment | Fix the root cause (for example, a failing agent script or missing prerequisite), then run `vizier jobs retry <job-id>` using the failed/blocked job id as the retry root. Vizier rewinds that job and downstream dependents, clears scheduler-owned runtime artifacts/logs, and re-queues the segment. |
| Draft worktree creation fails | Vizier deletes the stub branch unless the plan file was already committed. Re-run `vizier draft` once the error is fixed. |
| `vizier approve` fails mid-run | Temp worktree path is printed; inspect it to salvage partially staged files, then rerun once corrected. The plan branch remains intact. |
| Merge conflicts | Resolve conflicts on the target branch, stage the files, rerun `vizier merge <slug> --complete-conflict`. Vizier reuses `.vizier/tmp/merge-conflicts/<slug>.json` to finalize and fails fast if no pending merge exists. |
| Need to resume after aborting Git’s merge | As long as the sentinel JSON still exists and `git status` is clean, rerunning `vizier merge <slug>` finishes the in-progress merge without repeating refresh/removal steps. |
| CI/CD gate failure | The merge commit and draft branch remain untouched. Inspect the printed script output, apply fixes manually or rerun with `--auto-cicd-fix` (requires an agent that supports auto-fixes), and retry `vizier merge <slug>` once the gate script exits 0. |
| Backend auto-resolution fails | Vizier warns and falls back to manual instructions. Resolve/stage/retry just like a normal Git merge. |

## End-to-end walkthrough

1. **Draft**
   ```bash
   vizier draft --file specs/ingest.md
   ```
   - Output shows a label/value block, for example:
     ```
     Outcome: Draft ready
     Plan: .vizier/implementation-plans/ingest-backpressure.md
     Branch: draft/ingest-backpressure
     ```
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
   - Vizier refreshes `.vizier/narrative/snapshot.md`, removes the plan doc, and merges into the detected primary branch (e.g., `main`).
   - Final output is a block such as:
     ```
     Outcome: Merge complete
     Plan: ingest-backpressure
     Target: main
     Merge commit: <sha>
     ```
   - Need the branch for a follow-up? Append `--keep-branch` to suppress the default deletion step.
   - Hit a conflict? Resolve it on `main`, stage the changes, and rerun `vizier merge ingest-backpressure --complete-conflict` to finish the stored merge without creating ad-hoc commits.

Throughout the process, Outcome lines and Auditor records cite the plan slug, affected files, and any pending commit gates so auditors can trace who approved what. Tie this workflow into the broader agent-orchestration story by referencing this document in `AGENTS.md` or external SOPs when onboarding third-party agents.

## FAQ

**Can I run `vizier approve` without re-drafting?**  
Yes. If you manually edit `.vizier/implementation-plans/<slug>.md` on the draft branch, rerun `vizier approve <slug>` to reapply the plan. Vizier refuses to commit if the agent makes no changes so you can iterate safely.

**What if the draft branch lags far behind the target branch?**  
`vizier approve` prints a warning when `draft/<slug>` is missing commits from the target. You can merge or rebase the draft branch manually before running `vizier merge`, or accept that `vizier merge` may surface conflicts which you’ll resolve with the sentinel + `--complete-conflict` workflow described above.

**Does `vizier merge` push to origin?**  
Not automatically. Pass `--push` (global CLI flag) after `approve` or `merge` if you want Vizier to push once the command succeeds, or push manually when you’re ready.

## Architecture docs & agent workflows

- Plan documents live under `.vizier/implementation-plans/` and describe how the selected agent intends to implement the spec, but they do **not** replace architecture-doc references required by the compliance gate. Keep your architecture doc path handy so `vizier save` can cite it when the plan’s changes land on the primary branch.
- When multiple agents collaborate, use this workflow as the runbook: one operator drafts, another approves, and a third merges. The Auditor already records Outcome lines for each command, so reference those facts (plus the plan slug) inside `.vizier/narrative/snapshot.md` and narrative updates to keep the threads aligned.
