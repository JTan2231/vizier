---
plan: removing-wire
branch: draft/removing-wire
---

## Operator Spec
we need to remove wire in its entirety. all mentions, paths, and dependencies of it need scrubbed from the config and our code. we don't use the wire anymore. we don't support wire functionalities. this is a breaking change without replacement

## Implementation Plan
## Overview
- Remove the wire backend and all related flags, config keys, deps, docs, and tests so Vizier only supports script-based backends (Codex/Gemini shims or custom agents). This is a breaking change for operators who still target `backend=wire` or wire-only flags like `--model`.
- Goal is a cleaner, agent-only posture with no dead code or config traps; operators get fast failure plus updated guidance instead of silent fallback.
## Execution Plan
1) **Decide behavior changes and migration story** – Lock the post-wire backend matrix (agent/gemini only), define how CLI/config should react to `backend=wire`, `fallback_backend`, `--backend wire`, `--model`, and `--reasoning-effort` (likely hard errors with clear guidance), and record the breaking-change notes to flow into docs and snapshot/TODO updates.
2) **Purge wire from config/CLI surfaces** – Remove BackendKind::Wire and related parsing/help text; strip wire-only flags/options; adjust config parsing, defaults, and precedence (global/repo/CLI) to ignore or fail on wire keys; update example-config and prompt-config matrix to reflect agent/gemini-only settings.
3) **Rewrite core message/tool plumbing off wire types** – Introduce internal message/tool structs and prompt builders to replace `wire::types::*` plus `wire::api::Prompt`; rework Auditor/display/tool helpers to run entirely through AgentRunner/ScriptRunner flows; drop wire-specific request paths and model/resolution handling while keeping stream/status handling intact.
4) **Normalize agent runtime resolution after wire removal** – Simplify `AgentSettings`/runner resolution to only produce agent/gemini runners; ensure scope capability checks, prompt resolution, and metadata/session logging no longer reference wire and still honor documentation/prompt overrides.
5) **Dependency and doc cleanup** – Remove wire deps from Cargo manifests and any shim scripts; scrub README, AGENTS.md, workflow docs, and prompt-config-matrix of wire references, adding explicit breaking-change/migration guidance and updated examples.
6) **Test and thread alignment** – Update/replace unit and integration tests that assert wire behavior (backend selection, model override, conflict handling, CICD gate scenarios) with agent/gemini equivalents and new failure cases for wire configs; run `cargo fmt && cargo clippy && cargo test` across crates; verify no `wire` mentions remain (repo-wide search) and refresh snapshot/TODO threads to reflect the new agent-only posture.
## Risks & Unknowns
- Replacing `wire::types` may ripple through Auditor, display, and tool codepaths; need to ensure prompt construction and token-usage reporting stay consistent.
- Decision on `--model`/`--reasoning-effort`: removing or redefining these flags could surprise users; must choose clear errors vs. new semantics for agent/gemini.
- Existing configs/tests depend on wire; migration errors must be informative to avoid blocking operator workflows.
- Potential hidden wire assumptions in merge/review/check orchestration that could regress if not surfaced by tests.
## Testing & Verification
- `cargo fmt`, `cargo clippy`, and full `cargo test` for `vizier-core` and `vizier-cli`; rerun integration suite in `tests/src/lib.rs`.
- New/updated tests covering: rejecting `backend=wire` (config and CLI), agent/gemini backend resolution, prompt/profile precedence without wire, and plan/approve/review/merge flows post-removal.
- Manual `rg wire` (or equivalent) to confirm code/docs/config/examples are wire-free.
- Smoke the draft→approve→merge workflow with agent backend to confirm progress/history, session logging, and Outcome epilogues still behave.
## Notes
- Narrative shift: retire the wire backend entirely, move to an agent/gemini-only stack with explicit migration errors and refreshed docs/snapshot threads.
