# Feature Spec: Repo-Local `vizier develop` via Composed Workflow Templates

## Summary
Add a repo-defined `vizier develop` workflow that is equivalent to running:

1. `vizier draft`
2. `vizier approve`
3. `vizier merge`

The workflow must reuse existing scheduler, gate, and capability infrastructure, and it must be configured from repository files (not shipped as a built-in default).

## Goals
- Support repo-local composition of existing primitives without adding new domain behavior for draft/approve/merge.
- Keep orchestration auditable through existing job metadata, schedule DAGs, outcomes, and retry flows.
- Allow teams to evolve their develop pipeline by editing repository templates.
- Keep built-in wrapper commands (`draft`, `approve`, `merge`, etc.) unchanged and fully supported.

## Non-Goals
- Replacing or deprecating existing wrapper commands.
- Shipping `develop` as a global default in Vizier.
- Introducing new gate semantics beyond what the current workflow/capability system already supports.
- Designing a broad plugin marketplace in this phase.

## Desired UX
- Repository declares:
  - stage templates (for draft, approve, merge) as separate TOML files
  - a composition template (`.vizier/develop.toml`) that references those stage templates
  - a command alias mapping to the composition template
- Operator runs one command (`vizier develop ...` or `vizier run develop ...`) and gets the same net behavior as the three-command sequence, with normal scheduler visibility.

## Delivery Steps

### Step 1: Codify Stage Templates
Create repo-local stage templates for each primitive:

- `.vizier/workflow/draft.toml`
- `.vizier/workflow/approve.toml`
- `.vizier/workflow/merge.toml`

Requirements:
- Each stage template remains runnable as an independent template.
- Stage templates use existing canonical capabilities and contracts.
- Stage templates expose/consume the same artifacts and runtime args used by current wrappers (plan slug, draft branch, target branch, gate args, etc.).

### Step 2: Compose in `.vizier/develop.toml`
Define `.vizier/develop.toml` as a composition template that imports stage templates and links them in order.

Requirements:
- Composition expands to a single compiled DAG.
- Inter-stage dependencies are explicit and auditable.
- Artifact hand-off is explicit (draft outputs become approve inputs; approve outputs become merge inputs).
- Node ID collisions are prevented (for example via namespacing/prefixing at compile time).
- Template composition is cycle-safe (detect and reject import/edge cycles).

### Step 3: Runner and Resolution
Add generic alias execution and resolution so repo aliases become executable commands.

Requirements:
- Add a generic runner entry point (for example `vizier run <alias>`).
- Support alias resolution through `[commands.<alias>]` selectors first.
- Add repo-local file fallback for unmapped aliases (for example `.vizier/<alias>.toml`, then `.vizier/workflow/<alias>.toml`).
- Preserve scheduler behavior (`--after`, `--follow`, locks, dependencies, approvals, retries, job metadata).
- Preserve agent/prompt resolution semantics for nodes by capability/alias as implemented today.
- Optional UX sugar: route unknown top-level subcommands like `vizier develop` to alias runner resolution.

## Config Shape (Illustrative)
```toml
# .vizier/config.toml
[commands]
develop = "file:.vizier/develop.toml"
```

Stage and composition file names are repo-owned and can vary; the alias mapping is the contract.

## Behavioral Parity Requirements
- Running `develop` must be semantically equivalent to draft -> approve -> merge for the same inputs and config.
- Existing guardrails must remain active:
  - approval gates
  - stop-condition retries
  - merge conflict and CI/CD gate behavior
  - branch/worktree locking and scheduler dependency handling
- Retry must remain stage-aware and auditable through existing `vizier jobs retry` behavior.

## Acceptance Criteria
- A repository can define `.vizier/develop.toml` and run a single develop command end-to-end.
- `vizier jobs schedule` shows the composed DAG with stage ordering and dependencies.
- Failures in any stage halt downstream nodes consistently with current scheduler semantics.
- `vizier jobs retry <job-id>` can resume from failed develop-stage nodes using existing rewind semantics.
- `vizier plan --json` surfaces alias resolution for `develop`.
- Existing built-in commands continue to behave exactly as before.

## Open Questions
- Should `vizier develop` be introduced as first-class CLI sugar immediately, or only after `vizier run <alias>` stabilizes?
- Should composition syntax be an explicit `imports` primitive, or file-inlined expansion through selector indirection?
- What is the exact precedence between `[commands]` mapping and file fallback for top-level unknown commands?
- Should v1 include optional review stage composition (`draft -> approve -> review -> merge`) or keep strict three-stage parity?

## Implementation Plan
1. Define and lock composition schema for imported templates and inter-template edges.
2. Implement compile-time import expansion and cycle/collision validation.
3. Add generic alias runner with scheduler enqueue support through existing graph compilation paths.
4. Add top-level alias command dispatch (optional but recommended once runner is stable).
5. Add integration coverage for:
   - successful composed develop flow
   - stage failure propagation
   - retry semantics
   - alias/file resolution precedence
6. Update user docs/config reference to describe repo-local alias workflows and composition.

