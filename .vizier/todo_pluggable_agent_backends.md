# Pluggable agent backends for Vizier CLI

Thread: Agent backend abstraction + pluggable CLI agents

Tension
- Vizier currently treats `codex exec` as the only agent backend, so config, CLI flags, prompt orchestration, and progress reporting are all hard-wired to that binary (`vizier-core/src/codex.rs`, `vizier-core/src/lib.rs` prompts, `vizier-cli` actions). This makes it fragile and expensive to adopt new CLI agents like Claude or to run multiple agents side-by-side.
- Agent configuration is underspecified: some options only apply to specific backends (for example, model selection for the wire backend vs Codex), and there is no clear story for where agent config lives (global vs per-command vs workflow-specific) or how unsupported options are handled. Operators can’t easily predict which knobs will be honored, ignored, or rejected for a given agent.

Desired behavior (Product-level)
- Operators can choose which agent backend Vizier uses (Codex today, additional agents next) via config and/or a small set of CLI flags, without changing the draft → approve → review → merge choreography.
- Vizier exposes a stable “agent” interface that separates:
  - Transport/wire mechanics (how to invoke a CLI agent process and stream its JSON/progress) from
  - Agent semantics (capabilities, prompt shapes, per-agent options, and telemetry events).
- Plan, approve, and review flows (`vizier draft`, `vizier approve`, `vizier review`) call into the agent interface rather than a Codex-specific runner, so any compliant agent implementation can plug in and drive those workflows.
- Each agent backend can declare its capabilities (e.g., implementation-planning, code editing, conflict auto-resolution, review/critique) so Vizier can:
  - Enable richer flows where supported, and
  - Degrade gracefully (or fail clearly) when a requested feature is unavailable for the selected agent.
- Telemetry and progress reporting (history lines, token usage, errors) run through a common adapter so:
  - Outcome summaries and session logs expose a consistent story regardless of which agent handled the work, and
  - No backend-specific ANSI/JSON quirks leak past the abstraction.

Acceptance criteria
- Configuration and selection
  - There is a single, documented way to select the active agent backend (config key + optional CLI override), and it applies consistently across `vizier draft`, `vizier approve`, `vizier review`, and other assistant-backed commands.
  - Codex remains the default backend where available; selecting an unknown or misconfigured agent produces a clear, Outcome-reported error without starting partial workflows.
  - Agent configuration has a predictable scoping story: global config, per-command flags, and any workflow-specific defaults follow a documented precedence order (e.g., config < environment < CLI), and command-specific agent options stay small and coherent rather than exploding per-command flag sets.
  - When an operator supplies a config or flag that the selected backend does not support (for example, a model or reasoning knob that backend cannot honor), Vizier either:
    - Fails fast with an explicit Outcome reason that names the unsupported option and backend, or
    - Clearly reports that the option is ignored for this backend, so behavior never silently diverges from expectations.
- Agent interface and capabilities
  - Core exposes an agent interface that supports at least: implementation-plan generation, change-application for approve flows, review/critique for `vizier review`, and optional conflict auto-resolution for `vizier merge --auto-resolve-conflicts`.
  - Each backend can declare its supported capabilities; when a command depends on a missing capability (e.g., a review-only backend asked to apply fixes), Vizier either:
    - Refuses the operation with an explicit Outcome reason, or
    - Offers a documented degraded mode that requires no code edits from the agent (e.g., review-only).
  - The interface is documented at a product level (what behaviors are expected), not in terms of specific internal traits or type names, so future agents can implement it without guessing at hidden constraints.
- Prompt orchestration and flows
  - Prompt construction for plan/approve/review flows factors into agent-agnostic pieces plus backend-specific fragments where necessary, with a clear mapping from Snapshot/threads to prompts regardless of agent.
  - Existing Codex flows still work end-to-end via the new interface, with no regression in plan/approve/review behavior as described in the Snapshot and draft-approve-merge docs.
  - Adding a second backend (e.g., a CLI wrapper for Claude) can be validated via tests/integration without changing the draft/approve/review command semantics.
- Telemetry, progress, and Outcome integration
  - Progress history lines (e.g., `[agent:<name>] phase — message`) and token-usage reporting run through a backend-neutral adapter that respects the stdout/stderr contract, verbosity flags, and DAP/Outcome threads.
  - Session JSON artifacts under `.vizier/sessions/<id>/session.json` record which agent backend handled each operation, along with usage/telemetry when available, so auditors can distinguish Codex-driven vs. Claude-driven runs.
  - Outcome summaries (human epilogue + outcome.v1 JSON once implemented) include the selected agent backend and do not expose backend-specific wiring details (e.g., raw CLI invocation strings).
- Tests and safety
  - Integration tests cover at least: default Codex backend behavior, selecting an alternate test/dummy backend, capability-based failure/degrade paths, and Outcome/session logging that correctly attribute the backend used.
  - Non-TTY/protocol-mode behavior honors the existing stdout/stderr contract for all supported backends; no backend leaks ANSI into non-TTY contexts or bypasses the Outcome epilogue.

Pointers
- Snapshot thread: Agent backend abstraction + pluggable CLI agents (Running Snapshot — updated; Pluggable agent posture).
- Existing Codex wiring: `vizier-core/src/codex.rs`, `vizier-core/src/lib.rs::SYSTEM_PROMPT_BASE`, `vizier-core/src/lib.rs::IMPLEMENTATION_PLAN_PROMPT`, `vizier-cli/src/actions.rs`.
- Related threads: Agent workflow orchestration, stdout/stderr contract + verbosity, Outcome summaries, Session logging JSON store.

Update (2025-11-17)
- Added scoped agent configuration with `[agents.default]` plus per-command tables (`ask`, `save`, `draft`, `approve`, `review`, `merge`) so operators can select wire vs Codex per workflow. CLI overrides now sit at the top of the precedence chain and produce warnings when `--model` is ignored because the resolved backend is Codex.
- `vizier-core` exposes `AgentSettings`/`CommandScope` helpers, threads them through all assistant-backed commands, and records the resolved backend/scope inside session logs and token-usage summaries. Codex prompt builders accept per-scope bounds overrides, and `vizier approve/review/merge` fail fast when assigned a backend that lacks the required capabilities (tests cover `[agents.ask]` overrides and the `--backend wire` rejection on approve).
- Docs (`README.md`, `AGENTS.md`, `docs/workflows/draft-approve-merge.md`) now describe the schema, precedence, and agent workflow tie-ins. Remaining work: capability discovery for third-party backends, backend-neutral progress events, and full Outcome JSON reporting across all commands.
