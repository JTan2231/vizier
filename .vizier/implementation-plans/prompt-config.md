---
plan: prompt-config
branch: draft/prompt-config
status: implemented
created_at: 2025-11-17T05:52:21Z
spec_source: inline
implemented_at: 2025-11-17T06:11:49Z
---

## Operator Spec
Vizier’s plan workflow still hard-codes the IMPLEMENTATION_PLAN_PROMPT, REVIEW_PROMPT, and MERGE_CONFLICT_PROMPT in the binary, so operators cannot tune or version these critical agent behaviors per repo or environment. We need to expose
  these prompts through the existing configuration/prompt-store mechanism (and .vizier overrides) so the draft → review → merge choreography can be customized without recompiling.

## Implementation Plan
## Overview
Plan, review, and merge-conflict prompts live as Rust constants today, so operators must recompile Vizier to tweak workflows. We’ll route those templates through the same prompt-store/config system that already powers the base + commit prompts, letting repositories drop in `.vizier/…_PROMPT.md` files or set `[prompts]` keys to steer Codex without touching the binary. This unlocks repo-specific choreography for the draft → review → merge pipeline and keeps multi-agent orchestration flexible.

## Execution Plan
1. **Extend the prompt store with plan/review/merge templates**  
   - Broaden `vizier-core/src/config.rs`’s `SystemPrompt` enum (or rename to a more general `PromptKind`) to add `ImplementationPlan`, `Review`, and `MergeConflict`.  
   - Teach `Config::default` to look for `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`, `.vizier/REVIEW_PROMPT.md`, and `.vizier/MERGE_CONFLICT_PROMPT.md` (using `tools::try_get_todo_dir()` like the base/commit templates) before falling back to the existing constants in `vizier-core/src/lib.rs`.  
   - Update the JSON/TOML parsing paths so `[prompts]` (and legacy uppercase keys) can override the three new variants; document the canonical key names (`prompts.implementation_plan`, `prompts.review`, `prompts.merge_conflict`).  
   - Keep `Config::get_prompt` as the single retrieval point so future agents/backends automatically inherit overrides.

2. **Use the configurable templates inside Codex builders**  
   - In `vizier-core/src/codex.rs`, swap the direct `IMPLEMENTATION_PLAN_PROMPT`/`REVIEW_PROMPT`/`MERGE_CONFLICT_PROMPT` references for values pulled from `config::get_config().get_prompt(…)`.  
   - Ensure the builder helpers continue to append `<codexBounds>`, metadata, snapshot, and TODO sections exactly as they do today so prompt overrides only change the operator-facing instructions.  
   - Audit any other workflow (e.g., conflict auto-resolution in `vizier-cli/src/actions.rs`) that references those builders to confirm no additional hard-coded strings remain.

3. **Document the override story and surface user-facing guidance**  
   - Update README’s “Extensible Prompts” section (and, if needed, `docs/workflows/draft-approve-merge.md`) to enumerate the new drop-in filenames and config keys so operators know how to tune each plan-phase prompt.  
   - Mention that changes are picked up when Vizier reloads configuration (fresh process) and that overriding these prompts is the supported way to customize draft/review/merge instructions.  
   - If AGENTS.md or other onboarding docs reference prompt customization, add short notes pointing to the same mechanism for plan workflows.

## Risks & Unknowns
- Prompt files are loaded when the config singleton initializes; if operators edit `.vizier/REVIEW_PROMPT.md` mid-session they must restart `vizier` to see the change. We should call this out in docs to avoid confusion.  
- `tools::try_get_todo_dir()` assumes commands run inside a repo; commands invoked from elsewhere won’t find `.vizier/…_PROMPT.md`, so we need to confirm the fallback constants keep working in those scenarios.  
- Allowing arbitrary templates means operators could strip required headings/formatting; we’ll rely on documentation to convey the expected structure because validating Markdown in the builder would add undue complexity.

## Testing & Verification
- Extend the existing config tests (`vizier-core/src/config.rs`) to cover JSON/TOML overrides for the three new prompt keys plus the `.vizier/…_PROMPT.md` file-loading path.  
- Add focused tests for `build_implementation_plan_prompt`, `build_review_prompt`, and `build_merge_conflict_prompt` that temporarily swap `config::set_config` with custom prompt text and assert the rendered prompt begins with the override while still including bounds, metadata, snapshot, and TODO sections.  
- Manual sanity check: drop a custom `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`, run `vizier draft …`, and confirm the emitted plan instructions match the override (similarly for `vizier review` critique output and `vizier merge --auto-resolve-conflicts`).

## Notes
- This work unblocks the “Agent workflow orchestration” and “Pluggable agent backends” threads by letting repositories describe workflow-specific tone/format without forking the binary. Coordinate with anyone touching prompt schemas to avoid drift once these files become operator-owned artifacts.
- Summary of this drafting turn: captured the prompt-config implementation plan so draft/review/merge prompts can be overridden via config and documentation/tests reflect the new capability.
