---
plan: config-collapse
branch: draft/config-collapse
---

## Operator Spec

• Current: agent launch resolves via config/CLI, infers/auto-discovers binaries (codex exec/gemini agents run), and Rust assembles command vectors with extra knobs (profile/bounds/extra_args), though execution goes through the script
  runner.
  Target: keep scopes/prompts but collapse runtime to two levers—agent=<label> picks a bundled shell shim (codex/gemini) or command=<path> uses a custom shell script; Rust just runs that script, and the script must honor the runner IO
  contract (stderr for progress/errors, stdout for final assistant text).
  Gap: strip legacy knobs/autodiscovery from config/CLI/parser/tests/docs, ship/install the default shims, and align logging/metadata with the script-only model plus explicit runner IO contract.

## Implementation Plan
## Overview
Collapse the agent runtime surface to a script-only model: each scope picks either a bundled agent shim by label (codex/gemini) or a user-supplied script path, and Vizier simply executes that script under the existing stderr/stdout contract. This reduces config/CLI complexity, removes autodiscovery and legacy knobs, and aligns telemetry with the pluggable-agent + config-first posture without changing snapshot/TODO discipline.

## Execution Plan
1) **Target surface + compatibility story**: Define the new agent runtime schema (label→bundled shim or explicit `command` path) and deprecate legacy fields (`agent-backend`, autodiscovery, profile/bounds/extra_args). Decide whether old keys are hard errors or logged deprecations; document migration in help/AGENTS/README/example-config with a concise mapping table and explicit IO contract (stderr progress/errors, stdout final text).
2) **Config + CLI plumbing**: Update `vizier-core/src/config.rs` to drop autodiscovery/inference paths and resolve runtime strictly from label/command; simplify `AgentSettings/AgentRuntimeOptions` accordingly and ensure per-scope prompts/docs stay intact. Prune CLI flags in `vizier-cli/src/main.rs`/parsing helpers (`--agent-backend/--agent-bin/--agent-profile/--agent-bounds-prompt`) and route any remaining overrides to the new fields; align resolution logs and session/Outcome metadata with the script-only model (label + command path, no backend auto-fallback).
3) **Runner + metadata alignment**: Trim runner inputs to match the new schema (no profile/bounds/extra_args), ensure `AgentRequest` metadata records the chosen label/command, and keep progress rendering consistent with the stdout/stderr contract (no ANSI leak; quiet/verbosity respected). Confirm session logs/Outcome continue to capture agent label and command while remaining backend-agnostic for wire scopes.
4) **Bundle + install shims**: Promote the codex/gemini shell shims from `examples/agents` to installed assets (or a well-defined path), wire label→shim resolution in config defaults, and update `install.sh`/packaging so shims are available on PATH (or referenced by absolute path) across platforms.
5) **Docs + samples**: Refresh README, AGENTS.md, `docs/workflows/draft-approve-merge.md`, and `example-config.toml` to show the two-lever runtime (label or command), remove references to autodiscovery/extra knobs, and restate the runner IO contract. Add a brief migration note for operators upgrading configs/CLI invocations.
6) **Tests + validation**: Update unit tests around config resolution and agent runner to the new schema (no autodiscover branches, label→shim, missing command errors). Adjust integration tests that assert CLI flags/config precedence to cover the simplified surface. Add a coverage point proving stderr progress rendering still respects verbosity/TTY with the new runtime inputs.

## Risks & Unknowns
- Breaking existing configs/CLI usage that rely on autodiscovery or profile/bounds/extra_args; need a clear deprecation/error strategy and migration guidance.
- Packaging/default-shim pathing across platforms may be brittle; installation location must be deterministic for tests and runtime.
- Downstream threads (pluggable agent backends, stdout/stderr contract, session logging) rely on consistent metadata; regression risk if label/command are not captured uniformly.

## Testing & Verification
- Unit: config resolution tests for label/command-only schema, rejection/deprecation of legacy fields, and deterministic error on missing command.
- Unit: agent runner tests ensuring stderr progress is emitted and stdout is captured as final text with quiet/verbosity gating intact.
- Integration: CLI flows (ask/draft/approve/review/merge) using bundled shim labels to verify successful runs; negative cases where no command/label is provided.
- Integration/docs: snapshot of `vizier --help`/AGENTS/README/example-config to confirm removed flags and updated guidance.
- Regression: existing CI matrix (`cargo test -p vizier-core`, `cargo test --all --all-targets`) plus any merge-gate tests to confirm unchanged behavior outside agent runtime resolution.

## Notes
- Narrative: drafted the config-collapse implementation plan to simplify agent runtime selection to labeled shims or explicit scripts while keeping snapshot/TODO discipline intact.
