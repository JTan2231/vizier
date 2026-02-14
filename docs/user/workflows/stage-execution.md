# Stage Execution

This page describes execution behavior for each stage in the draft -> approve -> review -> merge flow.

For gate details, failure recovery, and merge-conflict resume, see `docs/user/workflows/gates-and-conflicts.md`.

## High-level timeline

If your repo has a composed alias such as `develop`, `vizier run develop ...` executes the same stages as one scheduled DAG.
Otherwise, run the explicit flow:

1. `vizier draft <spec>`: create `draft/<slug>` and `.vizier/implementation-plans/<slug>.md` from the primary branch in a disposable worktree.
2. Optional plan edits: update `.vizier/implementation-plans/<slug>.md` on `draft/<slug>` if you need refinements.
3. `vizier approve <slug>`: apply the plan on `draft/<slug>` in an isolated worktree and commit branch changes.
4. `vizier review <slug>`: run checks, stream critique, and optionally apply fixes on the plan branch.
5. `vizier merge <slug>`: refresh plan-branch narrative state, remove the plan doc, and integrate into target branch.

Assistant-backed commands enqueue scheduler jobs. Use `--follow` to stream logs and `vizier jobs` to inspect status when detached.

## `vizier draft`: create the plan branch

Prerequisites:

- Editing-capable agent selected for the `draft` alias.
- Primary branch is up to date.

Behavior:

- Derives slug from spec (or `--name`) and creates `draft/<slug>` from primary branch.
- Uses disposable worktree under `.vizier/tmp-worktrees/<slug>-<suffix>/`.
- Writes plan markdown to `.vizier/implementation-plans/<slug>.md`.
- Commits plan on `draft/<slug>` with `docs: add implementation plan <slug>`.
- Removes temp worktree on success.

Flags to remember:

- `vizier draft "spec"`
- `vizier draft --file SPEC.md`
- `vizier draft --name hotfix-foo "..."`
- `vizier draft ... --after <job-id>`

## `vizier approve`: implement the plan safely

Prerequisites:

- Clean working tree.
- Editing-capable agent selected for the `approve` alias.
- Plan branch and target branch exist locally.

Behavior:

- Validates branch relationship against target.
- Creates temp worktree on plan branch.
- Runs implementation agent against stored plan document.
- Stages and commits resulting branch edits via Auditor message.
- Preserves staged canonical narrative outputs while trimming unrelated `.vizier/*` noise.
- Removes temp worktree on success; preserves it on failure for debugging.
- Streams `[agent:<profile>] phase - message` progress lines while running.

Flags to remember:

- `vizier approve <slug>`
- `vizier approve --target <branch>`
- `vizier approve --branch <branch>`
- `vizier approve -y`
- `vizier approve --require-approval`
- `vizier approve --no-require-approval`
- `vizier approve --stop-condition-script <path> --stop-condition-retries <count>`
- `vizier approve --after <job-id>`

Git effects:

- Commits only land on the plan branch.
- Target branch and current checkout remain unchanged.
- If no files change, approve aborts with `agent completed without modifying files`.

## `vizier review`: critique the plan branch

Prerequisites:

- Clean working tree.
- Editing-capable agent selected for the `review` alias.
- Plan branch available locally.

Behavior:

- Runs merge CI/CD gate once before critique and feeds result into review context.
- Runs configured review checks (default cargo check/test in Cargo repos).
- Streams check output.
- Streams Markdown critique to stdout and session log (no `.vizier/reviews` file).
- Optionally re-enters agent to apply fixes and commit on plan branch.
- Leaves plan front matter lean (`plan` + `branch`) without review-status mutation.

Flags to remember:

- `vizier review <slug>`
- `vizier review --review-only`
- `vizier review --review-file`
- `vizier review --skip-checks`
- `vizier review -y`
- `vizier review --target <branch>`
- `vizier review --branch <branch>`
- `vizier review --cicd-script <path> --cicd-retries <n>`
- `vizier review --after <job-id>`

## `vizier merge`: land the plan with metadata

Prerequisites:

- Clean working tree.
- Plan branch contains reviewed commits.

Behavior:

- Refreshes plan branch narrative state and removes `.vizier/implementation-plans/<slug>.md`.
- Checks out target branch and integrates the plan.
- Default mode is squash: one implementation commit plus `feat: merge plan <slug>` commit containing an `Implementation Plan:` block.
- `--no-squash` keeps legacy merge graph behavior.
- If plan history contains merge commits, squash mode requires `--squash-mainline <n>` or config equivalent.
- Handles merge-conflict resume via `.vizier/tmp/merge-conflicts/<slug>.json` and `--complete-conflict`.
- Optionally auto-resolves conflicts when enabled and supported by selected agent.
- Deletes `draft/<slug>` on success unless `--keep-branch`.

Flags to remember:

- `vizier merge <slug>`
- `vizier merge --target <branch>`
- `vizier merge --branch <branch>`
- `vizier merge --squash` / `vizier merge --no-squash`
- `vizier merge --squash-mainline <n>`
- `vizier merge --auto-resolve-conflicts` / `vizier merge --no-auto-resolve-conflicts`
- `vizier merge --complete-conflict`
- `vizier merge --keep-branch`
- `vizier merge --cicd-script <path> --cicd-retries <n> --auto-cicd-fix`
- `vizier merge --after <job-id>`

## Operational helpers

- `vizier list [--target BRANCH]`: view pending `draft/*` branches ahead of target.
- `vizier jobs show <id>`: inspect `After`, dependencies, approval state, locks, and wait reasons.
- `vizier jobs retry <id>`: rewind failed/blocked segment and downstream dependents.
- `vizier completions <shell>`: generate shell completion scripts.
- `--no-commit`: hold workflow edits uncommitted for manual inspection (not supported by `vizier merge`).

Prompt customization for this workflow lives in command/template prompt tables in `.vizier/config.toml`; see `docs/user/prompt-config-matrix.md` and `docs/user/config-reference.md`.
