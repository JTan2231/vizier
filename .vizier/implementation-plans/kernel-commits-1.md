---
plan: kernel-commits-1
branch: draft/kernel-commits-1
status: draft
created_at: 2025-11-20T16:53:49Z
spec_source: inline
---

## Operator Spec
we want to rewrite our commit prompts to enforce kernel-style prompt writing. let's talk about what enforcement means: it means we write the prompt to guide the model to do so. _this is not a code-level enforcement_--this is the default guidance we provide to the models, and overrideable by the users through the existing prompt configuration. i want to emphasize--we are simply rewriting the prompts

## Implementation Plan
## Overview
We’ll rewrite Vizier’s default commit-message prompt so Codex (and other agent backends using the baked prompt) produces Linux kernel–style commits: `subsystem: summary` subject lines in imperative mood, hard 50/72 character limits, body paragraphs that emphasize the “why”, and required trailers like `Signed-off-by`. This responds to the operator spec and advances the “Git hygiene + commit practices” thread in `.vizier/.snapshot` by tightening default commit discipline without touching commit-generation code; operators can still override the prompt via the existing config/prompt store.

## Execution Plan
1. **Codify kernel-style guidance**
   - Review the kernel’s documented commit expectations (subsystem-prefixed subject, imperative mood, wrapping, problem/solution focus, `Signed-off-by`/`Fixes` trailers).
   - Decide which elements must be explicit in the prompt versus “nice-to-haves”, ensuring instructions remain concise enough for Codex.
   - Acceptance: we have a short checklist covering subject requirements, body focus, wrapping rules, and trailers to embed verbatim in the prompt.

2. **Rewrite the baked `COMMIT_PROMPT`**
   - Update `vizier-core/src/lib.rs::COMMIT_PROMPT` to drop the “conventional commit” language and instead enforce the kernel checklist (structure example, tone, wrapping, trailers, subsystem prefixes, “why over what” guidance).
   - Keep the wording strictly instructional (no code changes) and note that operators can override via `.vizier/COMMIT_PROMPT.md` or `[prompts.commit]`.
   - Acceptance: new prompt text matches the kernel checklist, compiles cleanly, and is the default returned by `PromptStore` lookups.

3. **Align docs/config references**
   - Search for repo docs that still claim the default is “conventional commits” (e.g., README, AGENTS.md, `example-config.toml` comments) and update phrasing so they now describe the kernel-style default plus reminder about overrides.
   - Acceptance: no repository guidance contradicts the new default; doc changes cite where overrides live.

## Risks & Unknowns
- Kernel-style nuances (e.g., when to add trailers beyond `Signed-off-by`) could be misquoted; mitigation: reference the official kernel guidelines while drafting the prompt.
- Some teams might still prefer conventional commits; ensure docs continue to mention the override mechanism so this change isn’t perceived as hard enforcement.
- If other tooling/tests assert on the old text (unlikely, but config tests reference prompt keys), we’ll need to update them alongside the prompt.

## Testing & Verification
- Run `cargo test -p vizier-core config` (or the full workspace tests if cheap) to ensure prompt-loading code and doc comments referencing the constant still compile after the string change.
- Optional: dry-run `cargo test -p vizier-cli prompt_store` (if such module exists) to ensure no assertions depend on the old verbiage.

## Notes
- No behavioral gates change here; this is strictly adjusting the baked guidance, keeping faith with the Git hygiene thread until broader commit-practice docs land.
