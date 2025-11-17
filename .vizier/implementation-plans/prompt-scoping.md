---
plan: prompt-scoping
branch: draft/prompt-scoping
status: implemented
created_at: 2025-11-17T17:33:06Z
spec_source: inline
implemented_at: 2025-11-17T18:12:11Z
---

## Operator Spec
we need to start scoping prompts to commands, similar to how we scope agents. if i want the base system prompt for ask vs. draft to be different, i should be able to specify in the config

## Implementation Plan
## Overview
- Operators need to scope prompt templates per Vizier command so “ask” and “draft” (and the other assistant flows) can run with tailored instructions, matching the existing agent-scoping story described in `.vizier/.snapshot` under “Pluggable agent posture”. This change primarily impacts repos that tune `.vizier/BASE_SYSTEM_PROMPT.md` or `[prompts]` in config; today every command shares the same base text, so workflows like `vizier draft` cannot differentiate their guardrails without editing the baked prompt.
- Scoping prompts by command keeps Codex-backed orchestration predictable as documented in README “Prompt overrides,” avoids fragile per-run overrides, and aligns with the operator request: “if i want the base system prompt for ask vs. draft to be different, i should be able to specify in the config.”
- Work touches `vizier-core/src/config.rs` (prompt store + session logging), `vizier-core/src/codex.rs` (prompt builders), `vizier-cli/src/actions.rs` (all Codex invocations), bootstrap flows, docs (`README.md`, `example-config.toml`, `AGENTS.md`), and the test suites covering config parsing and plan workflows.

## Execution Plan
1. **Scope + Precedence Design**
   - Inventory which commands consume which prompt kinds (`SYSTEM_PROMPT_BASE` for ask/save/approve/merge/plan refresh, Implementation Plan prompt for draft, Review prompt for `vizier review`, Merge-conflict prompt for merge auto-resolve, CICD failure prompt).
   - Define the configuration schema so operators can express per-command overrides without breaking today’s `[prompts]` table or `.vizier/BASE_SYSTEM_PROMPT.md` files. Proposed precedence (highest first): per-command config override ➜ repo-local `.vizier/<PROMPT>.md` ➜ global config entry ➜ baked default.
   - Document the schema up front (e.g., `[prompts.ask] base = """…"""`, `[prompts.draft] implementation_plan = """…"""`) and how it maps to `CommandScope` + `PromptKind`.

2. **Config + Prompt Store Extensions**
   - Extend `Config` with a `HashMap<(CommandScope, PromptKind), String>` (or nested map) to hold scoped overrides, plus helper structs for cleanliness.
   - Update `Config::default`, `set_prompt`, and the parsing paths in `from_value` to load both the existing global entries and any nested `[prompts.<scope>]` tables from TOML/JSON.
   - Add APIs:
     - `fn prompt_for(&self, scope: CommandScope, kind: PromptKind) -> String` returning the first matching override per the precedence above.
     - `fn prompt_source(&self, scope, kind) -> PromptSource` (struct capturing scope/kind/path/hash) for logging/session metadata.
   - Keep backwards compatibility: existing calls to `get_prompt` should either delegate to `prompt_for(CommandScope::Ask, kind)` or be replaced entirely where the scope is known.

3. **Prompt Builder + Session Metadata Plumbing**
   - Update Codex builders (`build_prompt`, `build_prompt_for_codex`, `build_implementation_plan_prompt`, `build_review_prompt`, `build_merge_conflict_prompt`, `build_cicd_failure_prompt`) to accept a `CommandScope` and pull text via `prompt_for`.
   - Update the non-Codex fallback `get_system_prompt_with_meta` so it also takes a scope and uses the scoped base prompt before appending `<meta>…</meta>`.
   - Refresh `Auditor::prompt_info` / `SessionPromptInfo` (`vizier-core/src/auditor.rs`) to record which scope+kind produced the current prompt and, when possible, the repo-relative path supplying it. This keeps `.vizier/sessions/<id>/session.json` accurate once multiple prompt variants exist.

4. **CLI & Bootstrap Wiring**
   - Thread the calling command’s scope everywhere we build prompts. `AgentSettings` already carries `scope`; use that when `vizier-cli/src/actions.rs` calls Codex (ask/save, plan refresh, plan implementation, review critique, review fixes, CICD remediation, merge auto-resolve, etc.) or when bootstrap (`vizier-core/src/bootstrap.rs`) synthesizes the snapshot.
   - Double-check nested flows (e.g., `apply_review_fixes`, `refresh_plan_branch`, `try_auto_resolve_conflicts`) to ensure they do not silently fall back to the wrong scope.
   - Ensure wire-backed paths (where `config::get_system_prompt_with_meta` used to hard-code the base prompt) now honor the same scoped selection, so scoping works regardless of backend.

5. **Docs, Samples, and Tests**
   - Update `example-config.toml`, README “Prompt overrides,” and `AGENTS.md` to describe the new `[prompts.<scope>]` tables, include examples for at least `ask` and `draft`, and spell out the precedence story so operators know how repo files interact with config.
   - Unit tests (`vizier-core/src/config.rs`) covering:
     - Parsing `[prompts.ask] base = "ASK"` overrides.
     - Fallback order (scoped override wins over global `[prompts]` and `.vizier/BASE_SYSTEM_PROMPT.md`).
     - Mixed prompt kinds (`implementation_plan`, `review`) per scope.
   - Integration/behavioral tests (likely in `tests/` alongside existing CLI workflows) verifying that:
     - Setting `prompts.ask.base = "Ask Scope"` results in `codex::build_prompt_for_codex` containing that text while `vizier draft` still uses the unmodified plan template.
     - Setting `prompts.draft.implementation_plan` changes the stored `.vizier/implementation-plans/<slug>.md` front-matter content for a draft to include the scoped text.
   - Adjust any mocks/fixtures asserting prompt hashes or text.

## Risks & Unknowns
- **Config compatibility:** Need to ensure new `[prompts.<scope>]` tables don’t collide with future config keys and that JSON parsing errors remain descriptive if operators typo a scope. Also confirm how inline strings vs. “load from file” expectations are communicated, since current config strings are literal prompt bodies.
- **Session logging accuracy:** Capturing the actual prompt source per scope may require extra metadata (e.g., there might be no file path if the prompt came from config). Decide on a stable encoding before implementation to avoid schema churn.
- **Scope coverage:** Commands like bootstrap, review fix-ups, or CICD remediation reuse the same agent but might deserve distinct prompt scopes in the future. For now we plan to reuse the calling command’s scope; confirm that’s sufficient or identify any new `CommandScope` variants needed.
- **Testing surface area:** Integration tests that depend on baked prompt text may start failing once prompts are configurable per scope; we’ll need to audit and update those assertions carefully.

## Testing & Verification
- **Config unit tests:** New cases in `vizier-core/src/config.rs` confirming TOML/JSON parsing for `[prompts.ask]`, fallback ordering, and map lookups for multiple `PromptKind` values.
- **Prompt resolution tests:** Add focused tests in `vizier-core/src/codex.rs` (or dedicated modules) to ensure `build_prompt_for_codex(scope=Ask)` pulls the scoped text while other scopes continue using defaults.
- **Integration tests:** Extend the existing plan/approve/review fixtures (`tests/` directory) to run `vizier draft` with a scoped implementation-plan prompt and assert the generated `.vizier/implementation-plans/<slug>.md` embeds the override; likewise run `vizier ask` with a scoped base prompt to verify the session log or Codex request contains it.
- **Session log validation:** Update tests covering `.vizier/sessions/<id>/session.json` to assert the stored `system_prompt` metadata now includes the scope/kind identifiers.
- **Doc linting:** Run doc build or lint checks if present to ensure README/AGENTS changes render correctly.

## Notes
- Narrative change: scoped the implementation plan to describe how Vizier will let operators override prompts per command.
