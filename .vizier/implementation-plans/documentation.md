---
plan: documentation
branch: draft/documentation
status: draft
created_at: 2025-11-21T17:31:46Z
spec_source: inline
---

## Operator Spec
Current: PromptKind::Base/SYSTEM_PROMPT_BASE is the mandatory “snapshot + TODO maintainer” prompt, injected into ask/save/approve/review-fix/pre-merge-refresh, always bundling snapshot/todo threads.
  Target: rename the base prompt to a “Documentation” (or similar) prompt and turn its use into a scoped, configurable toggle so commands can opt in/out of that narrative discipline.
  Also target: make inclusion of snapshot and TODO thread payloads configurable per scope, so flows can run without narrative context when desired.

## Implementation Plan
## Overview
- Add a configurable “Documentation” prompt (renamed from the current base/system prompt) so each command scope can opt in or out of the narrative discipline instead of always injecting it. Default remains opt-in to preserve DAP and snapshot/TODO upkeep.
- Make inclusion of snapshot and TODO thread payloads configurable per command scope, allowing flows like merge/approve/review-fix to run without narrative context when operators choose.
- Users impacted: operators tuning agent-backed commands, compliance reviewers relying on snapshot/TODO discipline, and downstream agents that consume prompt context. This is needed now to support pluggable agents and config-first posture without forcing narrative baggage on every run.

## Execution Plan
1) **Map current prompt wiring and scopes**
   - Trace where `PromptKind::Base`/`SYSTEM_PROMPT_BASE` is resolved and injected (ask/save/approve/review-fix/pre-merge-refresh) across `vizier-core/src/config.rs`, `vizier-core/src/agent_prompt.rs`, and `vizier-cli/src/actions.rs`.
   - Identify prompt file defaults (`BASE_SYSTEM_PROMPT.md`) and CLI/config override paths to plan backward-compatible rename handling.

2) **Introduce “Documentation” prompt kind with compatibility**
   - Rename the base prompt concept to “Documentation” (or similar) in `PromptKind`, default template, and repo prompt filename while providing migration/aliasing so existing configs/files still load.
   - Update prompt origin/selection metadata and session logging to emit the new label while honoring legacy `base` references for backward compatibility.

3) **Design per-scope toggles for documentation prompt use**
   - Add config switches (repo-local TOML + CLI flag if warranted) under the agent scope resolution so each command can enable/disable the documentation prompt injection explicitly; defaults mirror today’s always-on behavior.
   - Thread the resolved toggle through prompt building so commands marked off skip the documentation prompt entirely.

4) **Make snapshot/TODO payloads configurable per scope**
   - Add per-scope config for including snapshot and TODO thread payloads (separate booleans) with defaults on.
   - Refactor `agent_prompt::build_*` helpers to honor these toggles, ensuring empty/omitted sections follow the stdout/stderr and protocol-mode IO contract (no stray tags when omitted).

5) **Update wiring in CLI flows**
   - Adjust ask/save/approve/review/merge code paths to request the documentation prompt + context based on the new scope settings, keeping default behavior unchanged.
   - Ensure non-documentation flows still pass required bounds and operator spec data; avoid regressions in auditor gates, session logging, and plan drafting.

6) **Docs and examples**
   - Document the new prompt name, toggles, defaults, and precedence in README, AGENTS.md, `example-config.toml`, and any relevant workflow docs.
   - Call out how to opt out safely (and consequences for DAP/snapshot upkeep) to align expectations with compliance threads.

7) **Validation and tests**
   - Add unit coverage in config/prompt-resolution for legacy vs new prompt names and toggle precedence (CLI → scoped config → defaults).
   - Add prompt-builder tests for include/exclude combinations of documentation/snapshot/TODO payloads.
   - Extend integration tests (tests/src/lib.rs) to exercise a scope with documentation off and snapshot/TODO payloads disabled, asserting runtime behavior and session log attribution.

## Risks & Unknowns
- Backward compatibility: existing configs/files referencing `base` may break if the rename isn’t carefully aliased; plan includes migration/alias handling.
- Governance drift: allowing opt-out could weaken DAP/snapshot discipline; mitigate with default-on and explicit Outcome/session messaging about disabled context.
- Prompt shape expectations: downstream agents/tools may assume snapshot/TODO tags exist; need to confirm consumers tolerate omission.
- Scope creep: deciding which commands should ever skip documentation might require product guidance; will flag if defaults need steering.

## Testing & Verification
- Unit tests for prompt resolution covering legacy `base` vs new `documentation` names, per-scope toggles, and payload include/exclude.
- Agent prompt builder tests asserting snapshot/TODO sections appear or are omitted per config.
- Integration run through ask/save (defaults on) versus a scope configured off, verifying outcomes/session logs note the prompt/payload choices and no regressions in snapshot/TODO mutation.
- Non-TTY/protocol-mode checks to ensure omitted sections don’t introduce malformed output.

## Notes
- Narrative change: documents and implements a configurable “Documentation” prompt with per-scope context toggles while keeping narrative discipline as the default.
