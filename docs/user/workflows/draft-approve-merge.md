# Draft -> Approve -> Review -> Merge Workflow

This guide is the stable landing page for Vizier's plan workflow. It routes you to the focused workflow pages without changing command behavior.

Installed references: `man vizier`, `man vizier-build`, `man vizier-jobs`, `man 5 vizier-config`, `man 7 vizier-workflow`.

Before running plan workflows in a repository, run `vizier init` once (or `vizier init --check` in CI) so durable narrative markers and required `.gitignore` runtime rules are present.

For the canonical non-agent `.vizier/*` material model (entities, state vocabulary, durability, and compatibility/recovery semantics), see `docs/dev/vizier-material-model.md`.

## Documentation map (frozen)

| Previous section in this guide | Primary location now |
| --- | --- |
| Queue Plan Pipelines with `vizier build` | `docs/user/workflows/alias-run-flow.md#build-and-patch-pipelines` |
| Compose a One-Command Plan Flow with `vizier run` | `docs/user/workflows/alias-run-flow.md#compose-a-one-command-plan-flow-with-vizier-run` |
| Agent configuration / selector resolution | `docs/user/workflows/alias-run-flow.md#agent-configuration-for-plan-commands-and-alias-runs` |
| High-Level Timeline | `docs/user/workflows/stage-execution.md#high-level-timeline` |
| `vizier draft`: create the plan branch | `docs/user/workflows/stage-execution.md#vizier-draft-create-the-plan-branch` |
| `vizier approve`: implement the plan safely | `docs/user/workflows/stage-execution.md#vizier-approve-implement-the-plan-safely` |
| `vizier review`: critique the plan branch | `docs/user/workflows/stage-execution.md#vizier-review-critique-the-plan-branch` |
| `vizier merge`: land the plan with metadata | `docs/user/workflows/stage-execution.md#vizier-merge-land-the-plan-with-metadata` |
| Failure & recovery playbook | `docs/user/workflows/gates-and-conflicts.md#failure-and-recovery-playbook` |
| CI/CD and stop-condition gates | `docs/user/workflows/gates-and-conflicts.md#gates-and-policy-checks` |
| Merge conflicts and resume | `docs/user/workflows/gates-and-conflicts.md#merge-conflicts-and-resume` |
| End-to-end walkthrough | `docs/user/workflows/gates-and-conflicts.md#end-to-end-walkthrough` |
| FAQ | `docs/user/workflows/gates-and-conflicts.md#faq` |
| Shipping a local release | `docs/user/workflows/gates-and-conflicts.md#shipping-a-local-release` |

## Workflow pages

- `docs/user/workflows/alias-run-flow.md`: build/patch entry points, composed `run` aliases, and agent selector/config resolution.
- `docs/user/workflows/stage-execution.md`: command-by-command execution behavior for `draft`, `approve`, `review`, and `merge`.
- `docs/user/workflows/gates-and-conflicts.md`: gate behavior, conflict recovery, failure playbook, walkthrough, and FAQ.

## Quick start

1. Draft a plan on a dedicated branch:
   ```bash
   vizier draft --file specs/my-change.md
   ```
2. Apply the plan:
   ```bash
   vizier approve my-change
   ```
3. Critique and optionally fix:
   ```bash
   vizier review my-change
   ```
4. Merge into target:
   ```bash
   vizier merge my-change
   ```

If your repository defines a composed alias such as `develop`, you can run the same flow as one DAG:

```bash
vizier run develop --name my-change "ship my change"
```

Use `vizier list` to inspect pending draft branches and `vizier jobs` to inspect queued/running workflow jobs.
