---
plan: we-need-to-add-tab-completions
branch: draft/we-need-to-add-tab-completions
status: draft
created_at: 2025-11-16T19:12:56Z
spec_source: inline
---

## Operator Spec
we need to add tab completions for the slugs in the approve and merge commands

## Implementation Plan
## Overview
Adding slug-aware tab completion to `vizier approve` and `vizier merge` removes one of the few remaining manual steps in the draft → approve → merge workflow. Operators (and automated agents) will be able to tab through the set of pending plan slugs directly from the shell, reducing copy/paste errors and speeding up branch orchestration—an important lift for the Agent workflow orchestration and compliance threads called out in the snapshot and docs/workflows/draft-approve-merge.md.

## Execution Plan
1. **Catalog available plan slugs for completion**
   - Extend `vizier-cli/src/plan.rs` with a helper (e.g., `PlanSlugInventory::collect()`) that enumerates current draft slugs by scanning `.vizier/implementation-plans/*.md` and validating that a matching `draft/<slug>` branch exists (reuse the git walk logic already used by `list_pending_plans` so we only surface actionable branches).
   - Return both the slug and any helpful metadata (branch name, summary) so downstream completion code can format hints without re-reading the repo.
   - Make this helper resilient: handle missing plan files, ignore merged branches, and keep output sorted for deterministic completions.

2. **Introduce completion plumbing in the CLI**
   - Add `clap_complete` (and `clap_complete::dynamic` if needed) to `vizier-cli/Cargo.toml`.
   - Create a new `completions` module that wraps `Cli::command()` and exposes both static script generation (for bash/zsh/fish) and a hidden runtime completer entry point that shells can call to ask for dynamic values. Wire it behind a hidden subcommand such as `vizier __complete --shell bash --current "$COMP_LINE"` so shells can delegate completion requests back to Vizier.
   - Ensure the completer runs in the repo root (use the existing `vcs::repo_root` guard) and uses the helper from step 1 to fetch slugs on demand.

3. **Wire approve/merge plan args to the completer**
   - Update the positional `plan` args in `ApproveCmd` and `MergeCmd` to register a custom value parser/completer that emits suggestions from the slug inventory while leaving parsing permissive (so manual overrides still work).
   - When generating completion scripts, teach the bash/zsh functions to call into the runtime completer whenever the cursor is positioned on the plan argument for `approve` or `merge`; return plain newline-delimited slugs (and optional descriptions) so shells can present them as tab completions.
   - Verify that the completer respects `--branch` overrides (still returning the slug list) and that it no-ops outside a repo (return empty set with a friendly message so shells fall back to default behavior).

4. **Document and surface the feature**
   - Update README “How to use me” or a short new subsection under the draft → approve → merge workflow to mention that tab completion exists and show how to install the shell snippets (`vizier completions --shell zsh >> ~/.zshrc` or similar).
   - If docs/workflows/draft-approve-merge.md already walks operators through the CLI, add a short “Quality-of-life” callout describing slug completion so the workflow guide stays accurate.

## Risks & Unknowns
- **Clap version support:** The repo is on `clap = 4.0`; dynamic completion helpers may require a newer patch and/or `clap_complete`. Confirm compatibility before coding and budget time for a safe version bump if needed.
- **Shell coverage:** Bash, zsh, and fish have different expectations for completion scripts. Make sure the generated scripts call back into `vizier` consistently; otherwise some shells may fail quietly.
- **Repo context detection:** Completion calls might run outside a repo (e.g., in a bare shell). The helper should degrade gracefully without spamming stderr, otherwise tab completion could feel broken.
- **Performance:** Enumerating slugs requires walking branches and reading plan files; keep the helper lightweight (cache results per invocation, avoid expensive git operations) so tabbing feels instantaneous.

## Testing & Verification
- Unit tests for the new slug inventory helper covering: normal repo with two slugs, plan branch without plan file, merged branches being filtered, and deterministic ordering.
- Integration test that creates a fake repo with `.vizier/implementation-plans` + `draft/*` branches, runs the hidden completion entry point (e.g., `vizier __complete plan-slug ""`), and asserts that the expected slugs are printed.
- Smoke tests (bash/zsh) that source the generated script and ensure `vizier approve <TAB>` yields known slugs; for CI, this can be approximated by invoking the completer directly rather than running an interactive shell.
- Verify documentation renders correctly and references the new command/flags.

## Notes
- Coordinate with the Agent workflow/orchestration thread so any future workflow tooling (e.g., plan trackers) can reuse the same slug inventory instead of reimplementing the branch scan.
- If a hidden `__complete` command is added, ensure it stays undocumented in user-facing help but is mentioned in AGENTS.md or developer docs for maintainers who need to troubleshoot completions.
