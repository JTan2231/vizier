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
- Docs (README, docs/user/workflows/draft-approve-merge.md) describe the configuration behaviors in terms of observable outcomes instead of enumerating every internal lever.

## Acceptance criteria
- Usage:
  - An operator can run `vizier draft`, `vizier approve`, and `vizier merge` for a common case (single primary branch, no special target) without specifying more than one or two flags, and the behavior matches the documented defaults.
  - Flag semantics for shared options (`--target`, `--branch`, confirmation/auto behaviors, branch cleanup) are consistent across the three commands and documented once, then referenced from each command.
- Configuration story:
  - There is a single, operator-facing description of how configuration for the draft/approve/merge workflow is determined (e.g., precedence between CLI flags and config entries) that matches actual behavior.
  - Plan-specific configuration is treated as part of a coherent component (or section) in docs/config, not as a scattered list of one-off options.
  - When future gates (architecture-doc enforcement, multi-agent orchestration) introduce new knobs for this workflow, they extend this configuration story instead of creating parallel mechanisms.
- UX/Docs:
  - `docs/user/workflows/draft-approve-merge.md` and README.md both explain the workflow’s configuration at a product level (defaults, common variants) without requiring readers to infer behavior from help text alone.
  - Help output for `vizier draft`, `vizier approve`, and `vizier merge` is consistent in structure and terminology for shared flags, and does not contradict the docs.
- Tests:
  - Integration tests cover at least: default behavior with minimal flags; overriding target branch/location through configuration vs CLI; and a scenario where conflicting configuration sources resolve deterministically as documented.

## Pointers
- CLI surfaces and flags: `vizier-cli/src/main.rs`, `vizier-cli/src/actions.rs`
- Workflow docs: `docs/user/workflows/draft-approve-merge.md`
- Snapshot thread: Agent workflow orchestration (Running Snapshot — updated)

## Status
- Update (2025-11-16): Split the pending-plan listing flow into its own `vizier list [--target BRANCH]` command (hidden/deprecated `vizier approve --list` now just calls into it with a warning) and refreshed README + workflow docs/tests accordingly so operators don’t need to memorize an extra approve flag. Continue tightening the rest of the configuration story (target detection precedence, branch cleanup toggles, doc gates) in future iterations.
Update (2025-11-21): `vizier list` output now uses a count header plus Plan/Branch/Summary label blocks (sanitizing summaries and spacing entries) with a single Outcome block for the empty state; integration tests cover empty/multi-plan formatting so docs/config stay aligned with the default presentation.
Update — Surface CICD gate metadata in Outcome
- Add: Outcome epilogue/JSON for `vizier merge` must include {cicd_gate:{script, retries, auto_fix, attempts, status}} and a per-attempt log summary when auto-fix runs. Session logs attach full stdout/stderr.
- Acceptance:
  1) On gate success: status=passed; draft branch deletion noted unless `--keep-branch`.
  2) On gate failure: status=failed; merge is aborted; draft branch preserved; exit code 10.
  3) Auto-fix attempts list commits created and final status.
- Cross: Outcome component; Agent workflow orchestration.

Update (2025-11-20): Default squash merge behavior and knobs
- `vizier merge` now defaults to replaying plan commits onto the target, soft-squashing them into a single implementation commit, then writing a follow-up `feat: merge plan <slug>` commit that embeds the stored plan under an `Implementation Plan:` block; the squash path produces a single-parent merge commit so `draft/<slug>` drops out of the target ancestry. Repositories can flip `[merge] squash = false` in `.vizier/config.toml` or operators can pass `--no-squash` to keep the legacy “merge straight from draft/<slug> history” behavior; `--squash` forces the new behavior even when config disagrees. README, `docs/user/workflows/draft-approve-merge.md`, and `example-config.toml` describe the flag/config story, and integration tests (`test_merge_default_squash_adds_implementation_commit`, `test_merge_no_squash_matches_legacy_parentage`) lock in the default parentage so operators can reason about commit graphs when tuning Git hygiene policies.

Update (2025-11-22): Squash mainline selection for merge-heavy plan branches
- Squash-mode merges now preflight plan branches that contain merge commits and require either `--squash-mainline <parent index>` (or `[merge] squash_mainline = <n>`) to choose the mainline or `--no-squash` to keep the original graph; ambiguous octopus histories abort early with guidance. README and `docs/user/workflows/draft-approve-merge.md` document the new flag/config, and integration tests enforce the guard (`test_merge_squash_requires_mainline_for_merge_history`, `test_merge_squash_mainline_replays_merge_history`, `test_merge_no_squash_handles_merge_history`).

Update (2026-02-13): Repo-local one-command orchestration shipped via `vizier run <alias>` and `[commands].develop = "file:.vizier/develop.toml"` in this repo. This reduces operator flag churn for the common draft→approve→merge path while keeping existing wrapper commands intact; docs now describe the alias path and its precedence/fallback behavior so teams can opt into composed flows without a new global command surface.
Update (2026-02-13, gate-repair follow-up): Repaired drift between docs/tests and the on-disk repo defaults by restoring the tracked develop alias bundle (`.vizier/config.toml` mapping plus `.vizier/develop.toml` and `.vizier/workflow/{draft,approve,merge}.toml`). Acceptance signal: `test_plan_json_surfaces_develop_alias_selector`, `test_run_develop_composed_workflow_succeeds_with_stage_chain`, and `./cicd.sh` now pass in this worktree.
Update (2026-02-13, retry gate fix): Repaired a follow-on drift where `.vizier/config.toml` had regressed to merge-gate-only settings and dropped `[commands].develop`; re-adding `develop = "file:.vizier/develop.toml"` restored `/commands/develop/template_selector` in `vizier plan --json` and brought the same acceptance set (`test_plan_json_surfaces_develop_alias_selector`, `test_run_develop_composed_workflow_succeeds_with_stage_chain`, and `./cicd.sh`) back to green in this worktree.


---
