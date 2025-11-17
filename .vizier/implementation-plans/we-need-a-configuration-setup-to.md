---
plan: we-need-a-configuration-setup-to
branch: draft/we-need-a-configuration-setup-to
status: implemented
created_at: 2025-11-17T05:55:59Z
spec_source: inline
implemented_at: 2025-11-17T06:24:33Z
---

## Operator Spec
we need a configuration setup to support different commands for different agents. it should be as flexible as adding config items as flags in the CLI, and scopable to either _all_ commands or individual commands (ask, save, merge, approve, draft, review), with the most granular taking precedence.

## Implementation Plan
## Overview
Operators need to choose different agent backends or option sets for each Vizier command (ask/save/draft/approve/review/merge) without rewriting workflows. Today, `vizier-core/src/config.rs:75-222` exposes only process-wide knobs (single backend, Codex options, model), and `vizier-cli/src/main.rs:720-880` applies those values uniformly, so teams cannot, for example, run `vizier ask` via a lightweight HTTP agent while forcing `vizier approve`/`merge` through Codex. This blocks the Snapshot thread on pluggable agents (see `.vizier/todo_pluggable_agent_backends.md`) and complicates multi-agent orchestration. The change will introduce a scoped configuration story where operators can declare agent settings globally or per command, and CLI flags continue to override the most specific scope. Users impacted include anyone configuring Vizier for different agent backends, plus downstream auditors who consume session metadata.

## Execution Plan
1. **Define the scoped agent configuration model**
   - Inventory every command that talks to an agent today (`ask`, `save`, `draft`, `approve`, `review`, `merge`, plus internal helper flows called from `actions.rs`) and capture the capabilities each expects (plan generation, code edits, critique, merge auto-resolve).
   - Design a schema in prose for `[agents.default]` plus `[agents.<command>]` tables in `config.toml` that mirror the existing flags (`backend`, `fallback_backend`, `model`, `reasoning_effort`, Codex binary/profile/bounds, agent-specific extra args). Document precedence: CLI flag (global or future command-specific) > per-command config > `[agents.default]` > legacy top-level keys, so adding new CLI/config knobs stays symmetrical.
   - Record the schema and precedence rules inside AGENTS.md (top-level instructions already point agent authors there) so future backends know where to plug in, and add an example snippet to README’s backend section.

2. **Extend `vizier-core` config parsing and resolution**
   - Introduce `CommandScope`/`AgentSettings` structs in `vizier-core/src/config.rs` that hold the resolved backend + provider info for a given command, while keeping current `Config` fields for backward compatibility.
   - Teach `Config::from_value` to hydrate a map of per-command settings from `[agents.default]` and `[agents.<cmd>]`, validate command names, and merge nested Codex options/model overrides. When only legacy keys are present, populate the new structures from the existing single-source values.
   - Add helper APIs such as `config::resolve_agent_settings(scope, cli_overrides)` so callers never manipulate the map manually, and update tests in `vizier-core/src/config.rs` to cover precedence (default only, per-command override, CLI override, invalid command names).

3. **Apply scoped settings in the CLI and agent runners**
   - Update `vizier-cli/src/main.rs` to determine the `CommandScope` before invoking actions (`Commands::Ask`, `Commands::Draft`, etc.) and call the new resolver to produce `AgentSettings` for that scope. Store CLI flag values in an override struct so the resolver can apply them uniformly.
   - Thread the `AgentSettings` into the command runners (`actions.rs`), so `run_draft`, `run_approve`, `run_review`, `run_save`, and `inline_command` pass the chosen backend, model, and Codex options into the auditor/agent invocation instead of re-reading global state. Ensure commands that require Codex (`run_draft`, `run_approve`, `run_review`) continue to enforce capability checks; if a config assigns them to an unsupported backend, fail with the existing “requires Codex” messaging plus the offending scope.
   - Adjust `vizier-core::auditor` (e.g., at `vizier-core/src/auditor.rs:672-748` and similar) to accept an `AgentSettings` parameter or query the scope-specific config, and make sure session logging/outcome payloads include the resolved backend per command.

4. **Update observability, docs, and integration coverage**
   - Amend README’s backend section and AGENTS.md to describe the new `[agents.*]` tables, cite command names, and clarify that new config keys mirror CLI flags (ensuring the “just add a config item” requirement is satisfied). Add a short pointer in `docs/workflows/draft-approve-merge.md` so plan workflows mention the per-command agent story.
   - Extend the integration tests under `tests/src/lib.rs` to cover at least: (a) `vizier ask` honoring an `[agents.ask]` backend override while `vizier save` sticks to the default, and (b) CLI `--backend wire` overriding `[agents.approve]` to fail fast because approve still requires Codex.
   - Ensure session logs (`.vizier/sessions/<id>/session.json`) and Outcome lines reflect the resolved backend so auditors can confirm which agent handled each stage.

## Risks & Unknowns
- **Backward compatibility**: Existing configs that only set `backend = "codex"` must continue working; the migration path (legacy keys populating `[agents.default]`) needs careful regression testing and documentation.
- **Capability mismatches**: Assigning a backend lacking implementation features (e.g., a review-only agent on `approve`) must fail loudly; we need to confirm every command has clear capability metadata.
- **Complex overrides**: Introducing per-command overrides increases the chance of conflicting CLI flags; we may defer command-specific flag additions but still need to ensure logs make it clear which scope won.

## Testing & Verification
- Unit tests inside `vizier-core/src/config.rs` for parsing precedence, invalid command sections, and merge logic.
- CLI integration tests in `tests/src/lib.rs` that run `vizier ask`/`vizier save`/`vizier approve` using temp configs with `[agents.*]` entries, asserting each command uses the intended backend and surfaces failures when capabilities are unmet.
- Sanity tests (or manual verification scripts) that inspect `.vizier/sessions/<id>/session.json` to confirm the recorded backend matches the command-specific selection.
- Documentation lint (linkcheck or markdown CI) to ensure new README/AGENTS sections reference valid anchors.

## Notes
- Narrative delta: scoped plan authored for per-command agent configuration; no files changed yet.
