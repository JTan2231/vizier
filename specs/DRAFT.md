# Feature Spec: `vizier draft`

## Status

- Current-state spec for the existing `vizier draft` command.
- Documents shipped behavior (not a redesign).
- Baseline date: 2026-02-14.

## Purpose

`vizier draft` turns an operator spec into a durable implementation-plan artifact on a dedicated branch (`draft/<slug>`), without mutating the operator's currently checked-out branch.

The command is the plan-entry stage of the draft -> approve -> review -> merge workflow.

## User Outcomes

1. An operator can provide a spec (inline, file, or stdin) and get a plan branch plus plan document.
2. The branch contains a canonical plan file at `.vizier/implementation-plans/<slug>.md`.
3. The operator can continue with `vizier approve <slug>` (or equivalent composed `vizier run` flow).

## CLI Surface

Command form:

```bash
vizier draft [OPTIONS] [SPEC]
```

Draft-specific options:

- `SPEC` (positional): operator spec text.
- `-f, --file <PATH>`: read spec text from file.
- `--name <NAME>`: override slug derivation.
- `--after <JOB_ID>` (repeatable): explicit scheduler predecessors.

Relevant global options:

- `--follow`: stream scheduled job logs until completion.
- `--agent <SELECTOR>`: per-run agent selector override.
- `--no-commit`: hold generated artifacts uncommitted for manual review.

Input resolution contract:

1. Exactly one spec source is accepted: positional text, `--file`, or stdin.
2. Supplying both positional text and `--file` is rejected.
3. If no positional/file is supplied and stdin is non-TTY, stdin is consumed.
4. Empty file/stdin inputs are rejected.

## Execution Model

`vizier draft` is scheduler-backed.

Foreground/operator invocation:

1. Command enqueues a workflow-template DAG (default built-in template is a single draft node).
2. Root job metadata is recorded under `.vizier/jobs/...`.
3. With `--follow`, logs are tailed and process exit matches followed job exit.
4. Without `--follow`, command returns after enqueue with job summary.

Background worker invocation:

- Scheduler executes the draft node (`cap.plan.generate_draft_plan`) and runs the draft implementation logic directly.

Default built-in template characteristics:

- Template selector: `template.draft@v1` unless overridden via `[commands].draft` / workflow template settings.
- Primary node id: `draft_generate_plan`.
- Produces scheduler artifacts:
  - `plan_branch:<slug> (draft/<slug>)`
  - `plan_doc:<slug> (draft/<slug>)`
- Locks:
  - `branch:draft/<slug>` (exclusive)
  - `temp_worktree:<job_id>` (exclusive)

## Functional Behavior

### 1) Slug and Branch Resolution

- If `--name` is supplied, it is sanitized.
- Otherwise slug is derived from the first six words of spec text.
- Normalization rules:
  - lowercase ASCII alphanumeric + dash separators
  - trim leading/trailing dashes
  - max length 32
  - fallback `draft-plan` if normalization empties the value
- Reuse protection:
  - checks both existing local branch `draft/<slug>` and `.vizier/implementation-plans/<slug>.md` in the current repo worktree
  - if taken, appends `-1..-5`, then short random suffix attempts

Branch name is always `draft/<slug>`.

### 2) Primary Branch Base

Draft branch is created from detected primary branch using this order:

1. `origin/HEAD` target (if local branch exists)
2. `main`
3. `master`
4. most recently updated local branch

Failure to detect a primary branch aborts the command.

### 3) Worktree and Plan Generation

- Creates disposable worktree under `.vizier/tmp-worktrees/<slug>-<suffix>/`.
- Selects an agent for alias `draft` and requires agent-capable backend.
- Uses prompt kind `implementation_plan` to request plan content from the agent.
- Renders canonical plan markdown and writes:
  - `.vizier/implementation-plans/<slug>.md` (inside draft worktree)
- Upserts plan state record:
  - `.vizier/state/plans/<plan_id>.json` (inside draft worktree)
  - records include source=`draft`, intent (`inline|file:<path>|stdin`), branch/work ref, target branch, status=`draft`, summary.

### 4) Commit Mode

Auto-commit mode (default unless `--no-commit` or `[workflow].no_commit_default=true`):

- Commits plan doc + plan state record on `draft/<slug>` with message:
  - `docs: add implementation plan <slug>`
- Removes temporary worktree on success.

Manual mode (`--no-commit` effective):

- Leaves draft worktree dirty/uncommitted for manual inspection.
- Does not remove the temporary worktree.

### 5) Output Contract

On success, command prints outcome block and full plan document.

Auto-commit outcome fields include:

- `Outcome: Draft ready`
- `Plan: .vizier/implementation-plans/<slug>.md`
- `Branch: draft/<slug>`

Manual mode outcome fields include:

- `Outcome: Draft pending (manual commit)`
- `Branch`, `Worktree`, and `Plan`

## Plan Document Contract

Generated plan file format:

```md
---
plan_id: pln_<uuid>
plan: <slug>
branch: draft/<slug>
---

## Operator Spec
<original operator spec>

## Implementation Plan
<agent-generated plan body>
```

Notes:

- Front matter intentionally remains lean (`plan_id`, `plan`, `branch`).
- Status fields are not embedded in plan front matter.

## Failure and Cleanup Semantics

1. If worktree creation or plan generation fails before plan commit:
   - temp worktree is removed
   - newly created draft branch is deleted
2. If failure happens after plan commit:
   - committed draft branch is preserved
3. If temp worktree cleanup fails after success:
   - command warns; operator can prune manually
4. Failed draft runs must not leave a partially written plan in the operator's current branch/worktree.

## Configuration and Prompting

Agent/prompt controls:

- Agent selector resolves through alias/template config, with CLI `--agent` override.
- `draft` uses prompt kind `implementation_plan`.
- Prompt text resolution follows alias/template/default prompt precedence.

Template controls:

- Wrapper mapping can point `draft` to built-in or file-backed selector via `[commands]`.
- Custom template must satisfy `cap.plan.generate_draft_plan` capability contract (`spec_text` or `spec_file`, valid `spec_source`).

Commit posture:

- `[workflow].no_commit_default` toggles default manual vs auto commit posture.

## Acceptance Criteria

1. Running `vizier draft --name smoke "ship draft flow"` creates branch `draft/smoke` containing `.vizier/implementation-plans/smoke.md` with correct front matter and sections.
2. Operator's current branch does not receive draft plan commit.
3. Draft requires an agent-capable backend and fails fast with actionable guidance when missing.
4. Spec source validation rejects invalid combinations/empty inputs.
5. On backend failure, command exits non-zero and does not leave partial plan docs in operator branch/worktree.
6. Scheduler dependencies from `--after` are honored before draft node execution.

## Existing Coverage Map

- `tests/src/draft.rs`
  - draft creates branch + plan
  - draft captures agent metadata/session info
  - draft surfaces backend failure
- `tests/src/background.rs`
  - scheduler behaviors, dependency gating, follow/no-follow flow
- `tests/src/list.rs`, `tests/src/approve.rs`, `tests/src/review.rs`, `tests/src/merge.rs`
  - downstream workflow compatibility with draft outputs

## Canonical References

- `vizier-cli/src/actions/draft.rs`
- `vizier-cli/src/cli/dispatch.rs`
- `vizier-cli/src/cli/resolve.rs`
- `vizier-cli/src/plan.rs`
- `vizier-cli/src/workflow_templates.rs`
- `vizier-kernel/src/workflow_template.rs`
- `docs/user/workflows/stage-execution.md`
- `docs/user/config-reference.md`
- `docs/user/prompt-config-matrix.md`

## Companion Prompt Doc

- `specs/DRAFT_PROMPTS.md`
