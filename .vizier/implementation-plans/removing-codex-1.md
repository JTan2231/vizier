---
plan: removing-codex-1
branch: draft/removing-codex-1
status: draft
created_at: 2025-11-20T20:10:33Z
spec_source: inline
---

## Operator Spec
every possible surface that references codex directly needs removed. codex is a backend option and the vizier is _not_ opinionated on the agent users should use to solve their problems.

## Implementation Plan
## Overview
The operator wants Vizier’s public story, tooling surfaces, and workflow messages to be agent-agnostic so users are not steered toward Codex. This work touches every human-facing artifact—Snapshot/TODO narratives, docs, CLI help, runtime logs, and test fixtures—that currently hard-code “Codex.” Success means operators can configure any backend while all guidance, prompts, and outputs describe the “configured agent backend” instead of a specific vendor.

## Execution Plan
1. **Rebaseline Snapshot & TODO threads**
   - Sweep `.vizier/.snapshot` and codex-referencing TODO artifacts (e.g., `.vizier/todo_agent_command_cicd_gate.md`, `.vizier/todo_agent_workflow_orchestration.md`, `.vizier/todo_configuration_posture_and_defaults.md`) to restate active threads in backend-neutral language (“agent backend abstraction,” “configured backend”) while preserving current intent references.
   - Update Acceptance checkpoints and Next moves bullets so they no longer cite Codex-specific behavior; highlight that progress/history/gates reference “selected backend.”
   - Acceptance: running `rg -n Codex .vizier` after edits yields only backend-implementation files (if any), and the narrative still cross-links to the same threads.

2. **Rewrite docs & onboarding surfaces**
   - Refresh `README.md`, `AGENTS.md`, and `docs/workflows/draft-approve-merge.md` to describe the workflow, `--no-commit`, review gates, and prompt overrides without naming Codex. Replace examples with generic language like “configured backend” or “editing-capable agent,” and, where a concrete backend example helps, mention multiple options rather than a single brand.
   - Update `example-config.toml` to show neutral `[agents.<scope>]` samples plus backend-agnostic knobs (binary path, profile, bounds prompt, etc.). If backend-specific settings remain necessary, move them under backend-keyed subtables and mark them as examples rather than mandatory Codex guidance.
   - Ensure any other markdown files surfaced by `rg -l Codex docs` are rewritten similarly.
   - Acceptance: `rg -n Codex README.md AGENTS.md docs/ example-config.toml` returns zero matches, and the rewritten docs still map directly to the same commands/flags.

3. **Generalize CLI options & help text**
   - In `vizier-cli/src/main.rs`, rename `--codex-bin/profile/bounds-prompt` to backend-neutral names (e.g., `--agent-bin`, `--agent-profile`, `--agent-bounds`) and update help strings/descriptions for `--backend` to emphasize capability requirements instead of Codex. Provide backward-compatible hidden aliases (old flag names) so existing scripts keep working without documenting them.
   - Update `BackendArg` doc comments and enums so help text lists available backends without implying preference; when more backends arrive, this enum already reads as “agent backend.”
   - Adjust `vizier-core/src/config.rs` (e.g., `CodexOptions`, `CodexOverride`) so config serialization exposes generic names; keep internal structs or add serde aliases if needed to avoid breaking existing configs while removing the literal string “Codex” from user-editable files.
   - Acceptance: `vizier --help` and subcommand help contain no Codex mentions, legacy flags continue to parse (tested via CLI integration test), and config parsing still loads prior TOML thanks to aliases.

4. **Neutralize runtime messages, progress tags, and env levers**
   - Update `vizier-core/src/display.rs` to replace `ProgressKind::Codex` with a backend-agnostic variant that carries the active backend name/scope so stderr renders `[agent:draft]` or similar for any backend.
   - In `vizier-cli/src/actions.rs`, `vizier-core/src/auditor.rs`, `vizier-core/src/bootstrap.rs`, and `vizier-core/src/tools.rs`, replace hard-coded “Codex” strings with capability-based messaging (e.g., “requires a backend that supports plan application; configured backend: {name}”). Inject backend metadata from `AgentSettings` so info/warning lines (“resolved conflicts,” “reported no changes,” etc.) describe “the configured backend” rather than Codex.
   - Rename user-facing env vars such as `VIZIER_FORCE_CODEX_ERROR` to `VIZIER_FORCE_AGENT_ERROR` (with alias support) and ensure any stderr/stdout output triggered by forced errors references the new terminology.
   - Acceptance: sample runs of `vizier draft/approve/review/merge` show `[agent:*]` progress entries, all info/warning/error strings avoid the word “Codex,” and forcing the backend error via the renamed env var still exercises the mock path.

5. **Update tests & automation**
   - Revise `tests/src/lib.rs` assertions that currently expect “Codex …” text so they validate the new phrasing (“backend requires editing capability,” `[agent:draft]` progress labels, new env var name, etc.). Where tests want to ensure the Codex backend path is used, assert on backend identifiers or capability flags rather than literal strings in terminal output.
   - Extend integration coverage to confirm deprecated CLI flags still function (e.g., pass `--codex-bin` and ensure a warning/alias path), and add a regression test that ensures `vizier --help` stays vendor-neutral.
   - Acceptance: `cargo test` (workspace or at least `vizier-cli` integration suite) passes with updated expectations, and tests fail if any CLI output reintroduces direct “Codex” mentions.

6. **Verification sweep**
   - Run `rg -n Codex` across the repo and ensure matches remain only in backend-implementation internals (e.g., `vizier-core/src/codex.rs`) or serde-compat alias tables; no docs, config templates, CLI help strings, or runtime messages should include the literal.
   - Execute representative CLI flows (draft → approve → review → merge, plus `vizier ask/save`) under both TTY and non-TTY harnesses to confirm the updated messaging still satisfies the stdout/stderr contract and that no residual Codex references leak into outcomes or session logs.

## Risks & Unknowns
- Renaming CLI flags/config keys can break automation; we need a compatibility plan (serde aliases, hidden Clap aliases, or deprecation warnings) to avoid surprise downtime.
- Some Snapshot/TODO references describe Codex-specific behavior; rewriting them generically must preserve the historical context for active threads so auditors still understand prior work.
- Backend capability gating currently checks `BackendKind::Codex`; we may need a new capability flag in `AgentSettings` to produce accurate errors without naming Codex explicitly.
- Removing “Codex” from documentation leaves no obvious place to describe backend-specific setup; consider whether a dedicated backend-configuration doc (per backend) should live elsewhere or whether repo maintainers expect to derive it from code.

## Testing & Verification
- `cargo test -p vizier-cli` (covers CLI flag parsing, integration flows, Codex mock paths, and ensures progress labels change as expected).
- `cargo test -p vizier-core display`/unit suites to confirm the revamped progress kinds still honor verbosity/TTY gating.
- Manual CLI smoke for `vizier draft/approve/review/merge` and `vizier ask/save` to inspect human outcomes and ensure no Codex wording survives.
- Repo-wide `rg -n Codex` check documented above to confirm only backend-implementation files contain the literal.

## Notes
- Narrative change summary: reposition all docs, config surfaces, and CLI/user output to describe “the configured agent backend” instead of promoting Codex, aligning with the pluggable-agent thread’s goal of backend neutrality.
