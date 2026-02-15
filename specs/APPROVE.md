# `vizier approve` Feature Spec

## Status

- Existing behavior specification.
- Scope: current `vizier approve` command semantics (queue-time + runtime), not a redesign.

## Purpose

`vizier approve` applies a drafted implementation plan on a plan branch in an isolated worktree, commits the result (unless `--no-commit`), and optionally enforces a retrying stop-condition gate.

## CLI Surface

Command-specific args:

- `vizier approve <PLAN>`
- `--target <BRANCH>`
- `--branch <BRANCH>`
- `-y, --yes`
- `--stop-condition-script <PATH>`
- `--stop-condition-retries <COUNT>`
- `--after <JOB_ID>` (repeatable)
- `--require-approval`
- `--no-require-approval`

Relevant global args:

- `--follow`
- `--no-commit`
- `--push`
- `--agent <SELECTOR>`

## High-Level Execution Model

1. Front-door invocation (`vizier approve ...`) is scheduler-backed.
2. The wrapper command validates/compiles template policy and enqueues workflow jobs.
3. The primary workflow node executes internal runtime behavior (`run_approve`) under `--background-job-id`.

## Queue-Time Contract

### Preconditions and prompts

- In non-TTY mode, `--yes` is required; otherwise enqueue is rejected.
- In TTY mode, when `--yes` is absent, Vizier prompts:
  - `Approve plan <slug> on <branch>?`
- Plan slug is sanitized using plan-name rules.
- Plan existence check:
  - Plan document exists on the resolved branch, or an active `draft` job for the same plan exists.
- Branch existence check:
  - Resolved draft branch exists locally, or an active `draft` job exists.

### Template resolution and validation

- Template selector resolves through command/template config mapping for `approve`.
- Built-in template defaults to `template.approve@v1`.
- Stop-condition gate settings are injected from:
  - config (`[approve.stop_condition]`) plus CLI overrides.
- Invalid template capability contracts fail before enqueue.

### Scheduling behavior

- Full compiled template DAG is enqueued.
- Root job binds to semantic primary node (`cap.plan.apply_once`).
- `--after` dependencies are recorded and deduplicated in-order.
- `--require-approval` sets a scheduler approval gate:
  - Initial job status can be `waiting_on_approval`.
  - Queue summary prints `Next: vizier jobs approve <JOB_ID>`.

## Runtime Contract (`approve_apply_once`)

### Branch and ancestry checks

- Resolve plan spec:
  - slug, branch (default `draft/<slug>`), target (default detected primary branch).
- If target already contains branch tip:
  - command returns success with `Outcome: Plan already merged` and no mutation.
- If plan branch is behind target:
  - warning is emitted; approve continues.

### Worktree behavior

- Create disposable worktree under `.vizier/tmp-worktrees/<slug>-<suffix>/`.
- Run all approve mutation logic inside that worktree.
- On success with auto-commit:
  - worktree is removed (best effort; cleanup warnings are non-fatal).
- On failure:
  - worktree is preserved for debugging, and path is printed.
- With `--no-commit`:
  - worktree is intentionally preserved with pending changes.

### Agent invocation behavior

- Requires an agent-capable selector for the `documentation` prompt kind.
- Agent instruction requires:
  - read `.vizier/implementation-plans/<slug>.md`
  - implement execution plan
  - update `.vizier/narrative/snapshot.md`
  - update `.vizier/narrative/glossary.md`
  - update other narrative docs as needed
  - stage resulting edits
- Progress streams to stderr as `[agent:<profile>] ...` events.

### Commit behavior

- If the resulting diff is empty:
  - approve fails with `Agent completed without modifying files; nothing new to approve.`
- Auto-commit mode:
  - stages all (`git add .`) in worktree
  - trims unrelated `.vizier/*` noise from staged set
  - preserves canonical narrative outputs
  - creates one commit on the plan branch
- No-commit mode:
  - leaves changes dirty/staged in the plan worktree
  - no commit is created
- Plan markdown (`.vizier/implementation-plans/<slug>.md`) remains a scratch artifact and is not part of approve commit output.

### Push behavior

- `--push` only pushes when auto-commit created a commit.
- Push is skipped (with info message) when:
  - `--no-commit` is active, or
  - no approve commit was produced.

## Stop-Condition Policy

### Inputs

- Script source:
  - config: `[approve.stop_condition].script`
  - CLI override: `--stop-condition-script`
- Retry budget source:
  - config: `[approve.stop_condition].retries` (default `3`)
  - CLI override: `--stop-condition-retries`

### Semantics

- Script runs in the approve worktree root.
- If script passes (`exit 0`):
  - approve succeeds.
- If script fails and retry path exists:
  - approve re-runs full plan application, then re-runs stop script.
- Retry budget is "extra retries after first attempt":
  - max attempts = `1 + retries`.
- If budget is exhausted:
  - approve fails and preserves worktree.

### Observability

- Session operations include:
  - `approve_stop_condition_attempt` (per attempt)
  - `approve_stop_condition` (final summary)
- Attempt details include status, exit code, duration, stdout/stderr (clipped).

## Template/DAG Details

Built-in `template.approve@v1` defines:

- Primary node: `approve_apply_once` (`vizier.approve.apply_once`)
  - needs artifact: `plan_doc`
  - produces artifact: `plan_commits`
  - gate: approval (`required` toggled by `--require-approval`)
  - optional script gate + retry policy when stop-condition configured
- Control node: `approve_gate_stop_condition` (`vizier.approve.stop_condition`)
  - scheduled after primary
  - routes `failed -> approve_apply_once`

Current compatibility detail:

- For built-in selector `template.approve@v1`, control node jobs are enqueued but run as compatibility no-op.
- Effective stop-condition loop execution remains enforced by primary-node runtime policy for parity.
- Custom/file-backed templates with proper capability contracts can execute full node behavior.

## Prompt + Config Resolution

- Prompt kind used for approve implementation runs: `documentation`.
- Selector resolution order uses alias/template config tables with legacy scope fallback.
- Key config surfaces:
  - `[agents.commands.approve]` and template-scoped agent overrides
  - `[approve.stop_condition].script`
  - `[approve.stop_condition].retries`
  - `[commands].approve` (primary template selector table)
  - `[workflow.templates].approve` (legacy fallback)
  - `[workflow].no_commit_default`

## User-Visible Outcomes

Success (auto-commit):

- `Outcome: Plan implemented`
- includes `Plan`, `Branch`, `Stop condition`, `Review`, and latest commit hash.

Success (no-commit):

- `Outcome: Plan pending manual commit`
- includes preserved worktree path.

No-op success:

- `Outcome: Plan already merged`

Failures:

- clear error message + preserved worktree guidance for partial/failing runs.

## Edge Cases and Error Conditions

- Missing plan slug: CLI error.
- Unsupported/deprecated flags (for example `--backend`, `--list`) are rejected by clap.
- Missing plan doc/branch without active draft job: enqueue rejected with draft guidance.
- Invalid stop-condition script path or non-file path: enqueue rejected.
- Agent backend/runtime failure: approve fails; no new branch commit is added.
- Stop-condition never passes within budget: approve fails; worktree preserved.

## Acceptance Coverage (Existing Tests)

Primary coverage exists in:

- `tests/src/approve.rs`
- `tests/src/background.rs`

Covered behaviors include:

- `--yes` requirement in scheduler mode.
- Workflow template metadata persisted on queued jobs.
- Built-in control-node enqueue behavior.
- Custom template DAG support and semantic primary node binding.
- Invalid template rejection before queue.
- Single combined commit behavior on success.
- Canonical narrative staging + `.vizier` noise trimming.
- Stop-condition pass, retry-then-pass, and retry-exhausted failure behavior.
- Explicit approval gate flow (`--require-approval` -> `vizier jobs approve`).

## Source References

- Queueing + wrapper behavior: `vizier-cli/src/cli/dispatch.rs`
- Approve runtime: `vizier-cli/src/actions/approve.rs`
- Stop-condition runtime policy: `vizier-cli/src/actions/workflow_runtime.rs`
- Gate script execution + session operations: `vizier-cli/src/actions/gates.rs`
- Template construction/resolve: `vizier-cli/src/workflow_templates.rs`
- CLI args + option surface: `vizier-cli/src/cli/args.rs`, `vizier-cli/src/cli/resolve.rs`
- Canonical docs:
  - `docs/user/workflows/stage-execution.md`
  - `docs/user/config-reference.md`
  - `docs/user/prompt-config-matrix.md`

## Companion Prompt Doc

- `specs/APPROVE_PROMPTS.md`
