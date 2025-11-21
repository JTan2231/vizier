---
plan: codex-rename-1
branch: draft/codex-rename-1
status: draft
created_at: 2025-11-21T00:20:29Z
spec_source: inline
---

## Operator Spec
codex.rs needs refactored to serve as a hub for any sort of agent. the only code we should have that is agent-specific should be display code--the emitted json for each agent is different, so we'll want to be able to decode it. but otherwise, in the code, we should have a generic agent trait that represents any sort of agent, with agent-specific implementations for displays and a generic fallback for when we don't have one implemented.

## Implementation Plan
## Overview
The pluggable-agent thread in `.vizier/.snapshot` calls for a stable interface that lets Vizier swap Codex out for other CLI agents without rewriting `draft → approve → review` flows. Today `vizier-core/src/codex.rs` (e.g., `vizier-core/src/codex.rs:34-848`) hard-codes the process-backed runner, request/response structs, and Codex-specific event decoding; every caller (`vizier-core/src/auditor.rs:949-1115`, `vizier-cli/src/actions.rs:1297-3221`) assumes Codex semantics. The operator spec tightens this by requiring `codex.rs` to act as a general agent hub with a backend-neutral trait, pushing agent-specific differences into display/event decoding only. This refactor keeps existing Codex behavior while paving the way for future process-backed agents and ties directly into the “Agent backend abstraction + pluggable CLI agents” and “Agent workflow orchestration” threads.

## Execution Plan
1. **Define a backend-neutral agent interface**
   - Create a new `agent` module (or expand `vizier-core/src/codex.rs`) that introduces `AgentRequest`, `AgentResponse`, `AgentEvent`, `AgentError`, and a trait such as `AgentRunner` describing `async fn execute(&self, request, progress_hook) -> Result<AgentResponse, AgentError>`.
   - Keep existing fields (prompt, repo root, profile, binary, output mode, model, scope) but rename them generically so non-Codex backends can reuse the struct; add optional metadata maps so future agents can pass custom knobs without changing the trait.
   - Convert `CodexError`/`CodexRequest`/`CodexResponse` into implementations of the trait (or wrappers) and expose type aliases for Codex-specific options so existing config (`config::AgentSettings` and `ProcessOptions`) compiles with minimal change.
   - Acceptance: unit tests cover trait default impls and ensure `build_exec_args` still produces `codex exec` args for Codex-backed requests.

2. **Isolate process execution from event decoding**
   - Extract the “run a binary, stream stdout/stderr, collect usage” logic currently in `run_exec` (`vizier-core/src/codex.rs:671-848`) into a backend-neutral executor that returns raw `AgentEvent` payloads and final assistant text; treat “Codex output JSON” as just one decoder.
   - Introduce an `AgentDisplayAdapter` trait (or similar) responsible for turning `AgentEvent` into `display::ProgressEvent`. Provide a `CodexDisplayAdapter` that mirrors today’s `CodexEvent::to_progress_event` (`vizier-core/src/codex.rs:101-176`) and a fallback adapter that simply surfaces the event type/message when no specialized adapter exists.
   - Wire `ProgressHook` so it now takes decoder parameters; when a new backend registers its decoder, the CLI will emit `[agent:<scope>]` lines with backend-specific phases while unknown backends still produce human-readable fallbacks.
   - Acceptance: display tests confirm `[agent:approve]` output stays unchanged for Codex events; fallback adapter logs generic `backend=<name>` entries when fed arbitrary JSON.

3. **Refactor Codex-specific logic into an `AgentRunner` implementation**
   - Rebuild the current `CodexRequest` constructor helpers (prompt builders, bounds loading, `build_exec_args`) to return the general `AgentRequest` plus a `CodexDisplayAdapter`.
   - Ensure token-usage extraction stays encapsulated inside the Codex runner implementation; expose it via `AgentResponse.usage` so the Auditor/session logging code path remains unchanged.
   - Keep mock support (`cfg(feature="mock_llm")`) by implementing the trait with the existing mock helpers; this maintains the integration-test story with no CLI changes.
   - Acceptance: cargo tests covering mocks (`vizier-core/src/codex.rs:901-980`) pass through the new trait.

4. **Update Auditor and CLI entry points to consume the new interface**
   - Replace direct references to `codex::CodexRequest`, `CodexModel`, `CodexOutputMode`, and `codex::run_exec` inside `vizier-core/src/auditor.rs:949-1115` and the CLI command flows (`vizier-cli/src/actions.rs` at the approve/review/draft/merge call sites) with the trait-based API.
   - When resolving `AgentSettings`, pick the correct `AgentRunner` implementation (currently only Codex) and its associated display adapter, so future backends only need to register their runner and decoder.
   - Ensure session logs and token usage still record backend + scope exactly once; update any serde structs if field names change.
   - Acceptance: `vizier approve <slug>` and `vizier review <slug>` integration tests continue to pass, and CLI progress output still shows `[codex] phase — message`.

5. **Document the new abstraction and configuration touch points**
   - Update `AGENTS.md` and `README.md` configuration sections to describe the `AgentRunner` concept, explicitly noting that only display adapters are backend-specific now. Highlight how future agents would plug in (new runner + optional display adapter) without touching CLI workflows.
   - Extend `example-config.toml` with a note that additional process-based agents can reuse the same trait and only need to supply binary/model knobs plus event decoder registration.
   - Acceptance: docs mention the new agent trait and fallback decoder, satisfying the pluggable-agent posture in the snapshot.

## Risks & Unknowns
- Touching `auditor` and CLI flows is high-impact; regressions could break every process-backed command. Mitigation: refactor incrementally (introduce trait + wrappers first, swap call sites once tests pass) and lean on integration tests for approve/review/merge.
- Non-Codex backends might need options we have not modeled (e.g., environment variables, streaming formats). Plan leaves `AgentRequest` extensible (metadata map) so future needs don’t force another rewrite.
- Event decoding differences could flood the CLI with noisy fallback output if a backend emits high-frequency events. Provide rate limits or grouping in the fallback decoder, and document how backend authors should implement a specialized adapter.

## Testing & Verification
- Unit tests:
  - Trait conversions (request/build_exec_args) and fallback decoder behavior.
  - Codex display adapter reproduces current `[codex]` lines for sample events (use payloads from `vizier-core/src/codex.rs` tests).
  - Mock runner returns expected usage/event data through the trait.
- Integration tests:
  - Existing `tests/src/lib.rs` coverage for `vizier draft/approve/review/merge` should run unchanged; add a regression test that injects a dummy backend with custom events to confirm fallback decoding doesn’t panic.
  - Session log assertions ensure backend metadata and token usage still match the snapshot contract.

## Notes
- Coordinate with the “Agent backend abstraction + pluggable CLI agents” thread once this lands so future efforts (capability probes, telemetry adapters) build on the new trait instead of stacking more Codex-specific patches.
- Defer prompting changes (`build_*_prompt` helpers) until after the runner abstraction is stable; they can continue to live beside the new agent hub because they are already backend-neutral.
