---
plan: flow-docs
branch: draft/flow-docs
status: implemented
created_at: 2025-11-15T16:42:28Z
spec_source: inline
implemented_at: 2025-11-15T16:46:57Z
---

## Operator Spec
we need documentation on how to use the new draft -> approve -> merge flow, and what each piece entails regarding git transformations. thorough, user-oriented documentation for usability and understanding

## Implementation Plan
## Overview
- Produce a repo-native guide that walks operators through the new `vizier draft → vizier approve → vizier merge` workflow, emphasizing what each command does to Git history and how reviewers can audit the results.
- Target audience: maintainers and agents who must confidently hand off between plan drafting, implementation, and merge while satisfying Vizier’s compliance gates (Pending Commit, architecture-doc linkage, `.vizier` hygiene).
- Motivation: README only sketches the commands; operators now need deeper, user-oriented documentation that exposes the branch/worktree choreography and merge metadata so they can reason about safety, recoverability, and approvals without reverse-engineering the CLI.

## Execution Plan
1. **Source-of-truth sweep**
   - Re-read `README.md`, `AGENTS.md`, and the active snapshot/TODO threads to confirm positioning (threads: Agent workflow orchestration, Architecture doc gate, Outcome summaries).
   - Inspect `vizier-cli/src/actions.rs`, `vizier-cli/src/main.rs`, and `vizier-core/src/vcs.rs` to restate the exact Git operations, temporary worktree locations, and metadata each command writes.
   - Acceptance signal: internal notes enumerate the concrete steps (branch names, commits, plan file paths, conflict sentinels) for draft/approve/merge so later sections can cite them accurately.

2. **Draft dedicated workflow doc skeleton**
   - Add a new Markdown doc (e.g., `docs/workflows/draft-approve-merge.md`) with sections for Overview, Workflow Diagram/Timeline, Command Details, Failure/Recovery, and FAQ.
   - Include anchors for future Agent workflow orchestration references; align tone with README’s “Who I Am/What I Can Do”.
   - Acceptance signal: skeleton checked into repo with headings mirroring operator spec requirements; reviewers can see the structure before prose lands.

3. **Document `vizier draft` end-to-end**
   - Capture prerequisites (spec source, slug naming, working tree cleanliness), how `draft/<slug>` is created, where `.vizier/implementation-plans/<slug>.md` lives, and how Codex runs inside a temporary worktree.
   - Detail Git effects: branch creation, `.vizier` commit, untouched operator checkout, clean-up of `.vizier/tmp-worktrees/`.
   - Acceptance signal: doc section lists observable artifacts (branch name, plan file path, commit hash) and the commands/operators can run to inspect them (`git branch`, `git log draft/<slug>`).

4. **Document `vizier approve` mechanics**
   - Explain disposable worktree usage, Codex passthrough output, automatic staging/commits on `draft/<slug>`, and flags (`--list`, `--target`, `--branch`, `-y`).
   - Outline Git mutations: plan branch updates, `.vizier` refresh, no merge to primary yet, how pre-existing staged work is preserved, and how failures roll back.
   - Acceptance signal: section includes a state table (Before/After) describing HEAD, index, and `.vizier` files plus how to resume/retry.

5. **Document `vizier merge` and conflict handling**
   - Describe metadata-rich merge commit (plan slug, branch, spec source, created_at, summary, notes), `.vizier` plan removal, and optional branch deletion.
   - Cover conflict sentinel creation under `.vizier/tmp/merge-conflicts/`, manual resolution workflow, and `--auto-resolve-conflicts`.
   - Acceptance signal: doc provides a step-by-step checklist from invoking `vizier merge` through verifying the non-FF merge on the primary branch, including git commands to inspect commits and clean up branches.

6. **Add an end-to-end operator walkthrough**
   - Provide a narrative example (e.g., feature request → `vizier draft` → review plan file → `vizier approve slug` → inspect branch diff → `vizier merge slug`) with callouts on when to involve humans, where to stash architecture-doc references, and which Outcome summaries to expect.
   - Highlight audit checkpoints that tie into Agent workflow orchestration and architecture-doc gate threads.
   - Acceptance signal: walkthrough references concrete command outputs/artifacts and clarifies decision points (approve plan, handle pending commits, confirm destructive merges).

7. **Cross-link core docs**
   - Update `README.md` Core Commands section to link the new workflow doc in both `vizier approve` and `vizier merge` entries (using “Learn more” anchors until DRAFT.md/APPROVE.md exist).
   - Add a pointer in `AGENTS.md` (or the living agent contract) so external agents know where to read about the Git choreography.
   - Acceptance signal: README renders links correctly at top-of-file, and AGENTS.md enumerates the workflow doc under its resources list.

8. **Editorial + compliance checks**
   - Run Markdown lint/format tools if configured, ensure anchors (`#draft-approve-merge`) match README links, and verify no instructions contradict snapshot threads (e.g., don’t imply unshipped protocol mode).
   - Acceptance signal: `rg --files docs | xargs markdownlint` (or equivalent check) passes; doc language mirrors shipped behavior and flags.

## Risks & Unknowns
- **Behavior drift**: Documentation must remain accurate if CLI semantics change (e.g., pending mode split, architecture-doc enforcement). Mitigation: rely on code inspection and call out assumptions in doc margins.
- **Audience overlap**: Operators vs. agents may need different detail depth; risk of overloading README. Mitigation: keep README concise, push deep content into the new doc with clear anchors.
- **Git safety nuance**: Without screenshots/log excerpts, users might misinterpret how plan branches interact with local branches. Mitigation: include explicit `git` commands readers can run to verify state after each stage.

## Testing & Verification
- Manual proofread for accuracy by cross-checking documentation statements against `vizier approve --help`, `vizier merge --help`, and observed Git logs.
- Link validation (`markdown-link-check` or equivalent) over README and new doc to ensure references resolve.
- Optional smoke run: execute the workflow in a scratch repo using the documented steps to confirm the described artifacts (plan file paths, merge metadata) appear as stated; note observations for reviewers.

## Notes
- Tie-ins: aligns with active `todo_agent_workflow_orchestration.md` by giving humans a concrete runbook today; future orchestration UI can reference this doc.
- Narrative summary: Authored an implementation plan for documenting the draft→approve→merge workflow so operators can trust the Git choreography.
