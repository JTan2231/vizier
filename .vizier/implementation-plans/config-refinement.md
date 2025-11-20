---
plan: config-refinement
branch: draft/config-refinement
status: draft
created_at: 2025-11-20T18:33:24Z
spec_source: inline
---

## Operator Spec
Today: backend/model live under [agents.default/<scope>] with Codex-only options and separate prompt tables/files, so stages pick models via scope agent while prompts are scoped in a parallel surface.
  Reality: plan/review/merge hard-code Codex flavors (gpt-5.1 vs gpt-5.1-codex), wire-only tool paths, and prompt-level model overrides don’t exist—only scope-level agents decide the model; commit prompts still use the provider model
  separately.
  Desired: treat every backend as a pluggable agent with a single scoped map tying prompt text + model + per-backend options (rendered as flags/env) per command/kind, capability-gated instead of Codex-gated, with prompt-level model/option
  overrides and a clean migration from the current split surfaces.

## Implementation Plan
## Overview
Operators currently juggle separate agent tables and prompt overrides (`README.md:103-110`) while commands like plan/review keep forcing fixed Codex models (`vizier-core/src/codex.rs:33-75`). The pluggable-backend thread (`.vizier/todo_pluggable_agent_backends.md:5-59`) and the config-first posture goals (`.vizier/todo_configuration_posture_and_defaults.md:9-31`) both call for a single scoped map that ties prompt text, backend selection, model/reasoning knobs, and backend-specific options together per command. This plan defines and lands that unified configuration surface so every assistant command can choose an agent/prompt profile coherently, capability gating becomes backend-agnostic, and the documentation/tests teach operators how to reason about the new knobs.

## Execution Plan
1. **Baseline & schema decisions**
   - Inventory how `[agents.*]` scopes and `[prompts.*]` overrides are resolved today by tracing the config loader (`vizier-core/src/config.rs:24-196`), the baked prompt files in `vizier-core/src/lib.rs`, and the CLI resolution stack in `vizier-cli/src/main.rs:1033-1123`.
   - Draft the new schema that defines a `ScopedAgentProfile` per `CommandScope` with nested `PromptProfile` entries keyed by `PromptKind` (base, commit, implementation_plan, review, merge_conflict). Each profile must support: backend choice, model/reasoning overrides, prompt path, backend-specific flags/env, bounds prompt, and metadata for capability gating.
   - Capture migration rules (legacy `[agents.*]` + `[prompts.*]` → new profile) and publish them in a short design note or code comment so reviewers can confirm the shape before coding.

2. **Config parser and data model**
   - Extend `vizier-core/src/config.rs` with new structs (`AgentProfile`, `PromptProfile`, `BackendOptions`) and parsing logic that merges CLI overrides → scoped agent profile → default profile, while allowing per-prompt overrides of model/options.
   - Add serialization-friendly descriptions so session logs/outcomes can cite the resolved profile. Include support for backend-specific payloads (e.g., `codex.args`, `codex.env`, `wire.timeout`) with validation hooks that error when unsupported keys appear.
   - Implement the legacy-to-new translation layer so existing `[agents.*]` and `[prompts.*]` inputs populate the new structs, emitting deprecation warnings when the old layout is used but still behaving identically.
   - Update `AgentSettings` to carry the resolved prompt metadata and backend options so downstream callers no longer need to fetch prompt paths separately.

3. **Backend capability & invocation plumbing**
   - Replace the fixed `CodexModel` enum (`vizier-core/src/codex.rs:33-75`) with model strings supplied by the resolved profile, and thread per-prompt overrides (e.g., review vs approve) into `CodexRequest`.
   - Introduce a backend-neutral trait (e.g., `AgentBackend`) that exposes `capabilities()` plus `invoke(prompt, options)` so commands can gate on required abilities (plan drafting, change application, review critique, merge conflict resolution) instead of assuming Codex is always available.
   - Ensure backend-specific options from config render into the subprocess invocation (args/env for Codex; wire client options for the `wire` backend), and record the resolved prompt/model/backends in the Auditor/session metadata for auditing.

4. **CLI and workflow integration**
   - Update every assistant command entry point (`vizier-cli/src/main.rs:1033-1123`) to request a `ScopedAgentProfile`/prompt-kind tuple rather than manual prompt file probing. Commands that need multiple prompts (e.g., `vizier draft` uses base + implementation_plan) should request each explicitly and allow per-kind overrides.
   - Replace the current “Codex-only” guards inside `vizier-cli` workflows and worktree helpers with capability checks, surfacing clear Outcome errors when an operator selects a backend that lacks a required capability.
   - Refresh CLI warnings (`warn_if_model_override_ignored`) so they point at the new schema (e.g., “model override ignored because prompt-profile foo forces backend=codex”) and add flag plumbing if new per-prompt overrides need CLI exposure.

5. **Docs, examples, and migration aids**
   - Rewrite the configuration sections in `README.md:103-118`, `docs/workflows/draft-approve-merge.md:1-120`, `AGENTS.md:1-13`, and `example-config.toml:1-80` to explain the unified profile syntax with concrete samples (showing per-command prompt/model overrides and backend options).
   - Document migration guidance (how legacy `[agents.<scope>]` + `[prompts.<kind>]` map to the new structure, what warnings to expect) and mention the change in the relevant TODO threads so the config-first posture story stays coherent.
   - Provide operator-facing release notes or a short `.vizier/implementation-plan` addendum that points maintainers to the new schema before they re-run `vizier draft/approve`.

## Risks & Unknowns
- **Migration complexity:** Existing repositories may rely on bespoke `[prompts.*]` files or Codex-only options; mis-parsing could silently change models. Mitigate with explicit warnings and integration tests that cover “old config only” vs “new schema.”
- **Capability matrix clarity:** Defining granular capabilities per backend may uncover edge cases (e.g., wire backend lacking merge auto-resolution). Need to ensure failure messages remain actionable and do not strand operators mid-command.
- **CLI surface creep:** Adding per-prompt overrides risks bloating flags; keep CLI overrides minimal and push most variability into config to honor the configuration-posture thread.

## Testing & Verification
- **Unit tests (config):** Add coverage in `vizier-core/src/config.rs` for parsing new profiles, legacy translation, per-prompt overrides, and backend-specific option validation (including error cases for unsupported keys).
- **Integration tests:** Extend `tests/src/lib.rs` to run representative commands with: (a) legacy config only, (b) new profile-only config, and (c) mixed overrides, asserting the resolved backend/model/prompt recorded in session logs matches expectations and that capability errors block unsupported backends cleanly.
- **Backend invocation tests:** Create focused tests/mocks for both Codex and wire paths verifying that profile-provided args/env/model reach the subprocess/client and that progress/token reporting still works.
- **Doc/help consistency checks:** Hook existing doc-lint tooling (or add a simple assertion) ensuring `example-config.toml` matches the schema described in README/AGENTS so future updates remain synchronized.

## Notes
- Coordinate landing with any pending work on agent orchestration and outcome JSON so the new profile metadata flows into those surfaces without follow-up churn.
