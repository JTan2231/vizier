---
plan: true-auto-resolve
branch: draft/true-auto-resolve
---

## Operator Spec
• Current state: merge conflict auto-resolution only triggers when the CLI flag --auto-resolve-conflicts is passed; without it (or when resuming via --complete-conflict), Vizier takes the manual path. Misalignment: .vizier/config.toml
  [merge.cicd_gate].auto_resolve is CI/CD gate remediation only and does not affect conflict handling, so auto-resolution wasn’t active in the failing run. Desired: a unified, predictable path where merge conflicts are auto-resolved
  whenever requested—either by honoring a config knob or making the flag default—and resumption behaves the same as the initial attempt.

## Implementation Plan
## Overview
- Align merge conflict auto-resolution with a predictable, config-first contract: honor a repo knob as the default, keep CLI overrides, and ensure resume flows behave the same as first-run.
- Decouple conflict auto-resolution from the existing CI/CD gate auto-fix setting to remove operator confusion and make the UX auditable.
- Users of `vizier merge` (agent-backed plan merges) gain clearer defaults, consistent resume behavior, and documentation/tests that match the advertised workflow.

## Execution Plan
1) **Clarify desired contract + defaults**
- Decide and document the intended default (stay opt-in or default-on) and where it lives (new `[merge.conflicts] auto_resolve_conflicts` or similar) without overloading `[merge.cicd_gate].auto_resolve`.
- Map CLI override precedence (`--auto-resolve-conflicts` / `--no-auto-resolve-conflicts` > repo config > built-in default) and confirm agent-backend requirement for auto-resolution.

2) **Wire config + flag resolution into merge + resume paths**
- Update `vizier merge` argument handling (vizier-cli/src/main.rs, actions.rs) to read the new/defaulted knob, apply CLI overrides, and pass a single boolean through to the conflict handler.
- Ensure the sentinel/resume path (`--complete-conflict`) reuses the resolved setting so auto-resolution attempts run (or stay off) consistently across retries.
- Keep CI/CD gate auto-fix plumbing untouched and clearly separated.

3) **Surface state in outputs and UX**
- Adjust merge progress/outcome messaging to state whether conflict auto-resolution was attempted, skipped, or unavailable (agent capability/flag), keeping stdout/stderr and mode-split contracts intact.
- Update help text and error paths to disambiguate conflict auto-resolve from CI/CD gate auto-fix.

4) **Docs + config examples**
- Refresh README and `docs/workflows/draft-approve-merge.md` with the new default/override story, resume behavior, and config key.
- Update `.vizier/config.toml`/`example-config.toml` and AGENTS/Docs prompts if needed to show the separation between conflict auto-resolve and CI/CD gate auto-fix.

5) **Tests + coverage**
- Add integration coverage in `tests/src/lib.rs` (or targeted unit tests) for: defaulted auto-resolve on first merge, config enabling when the flag is absent, CLI override disabling when config enables, consistent behavior on `--complete-conflict`, and no regression to CI/CD gate auto-fix.
- Include a failure path where auto-resolution is unsupported or fails, asserting clear Outcome/exit code and preserved sentinel for manual completion.

## Risks & Unknowns
- Changing the default could surprise teams who prefer manual conflict handling; may need a conservative default with explicit release notes.
- Agent capability gaps (backend lacking auto-resolve) must fail gracefully without blocking manual merges.
- Interaction with squash/merge-history guards needs validation so automated conflict edits don’t violate the two-commit contract.

## Testing & Verification
- Integration tests for merge conflict scenarios covering: config-on default, CLI off override, and resume parity.
- Test auto-resolve failure/unsupported backend path to ensure clear Outcome and preserved recovery.
- Regression tests that CI/CD gate auto-fix still behaves as before and is not triggered by the conflict knob.
- Doc/help checks to ensure new flags/config appear and no ANSI/verbosity regressions in progress/outcome lines.

## Notes
- Narrative alignment: clarifies the split between conflict auto-resolution and CI/CD gate remediation, and carries the new config-first default into merge/resume UX.
