# Tame configuration surface for draft → approve → merge workflow

## Thread
- Agent workflow orchestration

## Goal
Keep the configuration and flag surface for the `vizier draft → vizier approve → vizier merge` workflow small, predictable, and easy to reason about while the workflow becomes a core, high-traffic component. Make the “happy path” obvious with sensible defaults, and keep advanced levers discoverable but non-obligatory.

## Tension
- The draft/approve/merge commands already expose multiple flags and behaviors (e.g., `--list`, `--target`, `--branch`, `--delete-branch`, `--note`), with more knobs likely as architecture-doc gates and multi-agent orchestration land.
- Operators are starting to experience the configuration surface as “exploding,” which adds cognitive load right where we want the workflow to feel guided and safe.
- The workflow is about to become a central integration point for agents and humans; if its configuration feels ad-hoc, it will undercut trust in the orchestration story.

## Desired behavior (Product-level)
- The plan workflow (draft → approve → merge) presents a coherent, minimal configuration surface:
  - A clear, documented “default behavior” that works with zero or few flags for common cases.
  - Advanced options grouped and named so operators can infer effects without reading source.
  - Consistent flag semantics across the three commands (e.g., `--target`, `--branch`) with a single, well-documented precedence story for config vs CLI flags.
- Configuration relevant to plan workflows feels like a single component rather than scattered toggles (e.g., branch naming conventions, auto-delete policies, and doc/plan linkage all live under an identifiable configuration story, even if split across files today).
- Docs (README, docs/workflows/draft-approve-merge.md) describe the configuration behaviors in terms of observable outcomes instead of enumerating every internal lever.

## Acceptance criteria
- Usage:
  - An operator can run `vizier draft`, `vizier approve`, and `vizier merge` for a common case (single primary branch, no special target) without specifying more than one or two flags, and the behavior matches the documented defaults.
  - Flag semantics for shared options (`--target`, `--branch`, confirmation/auto behaviors, branch cleanup) are consistent across the three commands and documented once, then referenced from each command.
- Configuration story:
  - There is a single, operator-facing description of how configuration for the draft/approve/merge workflow is determined (e.g., precedence between CLI flags and config entries) that matches actual behavior.
  - Plan-specific configuration is treated as part of a coherent component (or section) in docs/config, not as a scattered list of one-off options.
  - When future gates (architecture-doc enforcement, multi-agent orchestration) introduce new knobs for this workflow, they extend this configuration story instead of creating parallel mechanisms.
- UX/Docs:
  - `docs/workflows/draft-approve-merge.md` and README.md both explain the workflow’s configuration at a product level (defaults, common variants) without requiring readers to infer behavior from help text alone.
  - Help output for `vizier draft`, `vizier approve`, and `vizier merge` is consistent in structure and terminology for shared flags, and does not contradict the docs.
- Tests:
  - Integration tests cover at least: default behavior with minimal flags; overriding target branch/location through configuration vs CLI; and a scenario where conflicting configuration sources resolve deterministically as documented.

## Pointers
- CLI surfaces and flags: `vizier-cli/src/main.rs`, `vizier-cli/src/actions.rs`
- Workflow docs: `docs/workflows/draft-approve-merge.md`
- Snapshot thread: Agent workflow orchestration (Running Snapshot — updated)

