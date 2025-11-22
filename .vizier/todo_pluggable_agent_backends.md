# Pluggable agent backends for Vizier CLI

Thread: Agent backend abstraction + pluggable CLI agents

Tension
- Vizier still ships with `codex exec` as the only concrete CLI agent backend: the new `AgentRunner`/`AgentDisplayAdapter` interface sits in front of that binary, but there is no plug-in story for alternate agent binaries yet, and progress/usage wiring still assumes Codex-style JSON events.
- Agent configuration now lives under `[agents.default]` and per-scope `[agents.<scope>]` tables plus CLI overrides, but there is still no capability-discovery story for future non-Codex backends or a clear way to signal when backend-specific options are unsupported; operators need predictable feedback when configuration knobs are ignored or rejected.

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

Update (2025-11-17): Prompt orchestration already flows through the shared config store for every phase (base, commit, implementation-plan, review, merge-conflict), so future backends automatically inherit repo-specific instructions. The transport/capability abstraction, backend-selection knobs, and telemetry adapters remain outstanding.

Pointers
- Snapshot thread: Agent backend abstraction + pluggable CLI agents (Running Snapshot — updated; Pluggable agent posture).
- Existing Codex wiring: `vizier-core/src/codex.rs`, `vizier-core/src/lib.rs::SYSTEM_PROMPT_BASE`, `vizier-core/src/lib.rs::IMPLEMENTATION_PLAN_PROMPT`, `vizier-cli/src/actions.rs`.
- Related threads: Agent workflow orchestration, stdout/stderr contract + verbosity, Outcome summaries, Session logging JSON store.

Update (2025-11-17)
- Added scoped agent configuration with `[agents.default]` plus per-command tables (`ask`, `save`, `draft`, `approve`, `review`, `merge`) so operators can select wire vs Codex per workflow. CLI overrides now sit at the top of the precedence chain and produce warnings when `--model` is ignored because the resolved backend is Codex.
- `vizier-core` exposes `AgentSettings`/`CommandScope` helpers, threads them through all assistant-backed commands, and records the resolved backend/scope inside session logs and token-usage summaries. Codex prompt builders accept per-scope bounds overrides, and `vizier approve/review/merge` fail fast when assigned a backend that lacks the required capabilities (tests cover `[agents.ask]` overrides and the `--backend wire` rejection on approve).
- Docs (`README.md`, `AGENTS.md`, `docs/workflows/draft-approve-merge.md`) now describe the schema, precedence, and agent workflow tie-ins. AGENTS.md includes a concrete configuration example with `[agents.default]`, per-command overrides like `[agents.ask]`, and nested `[agents.<scope>.codex]` settings so the precedence story is explicit for humans and downstream agents. Remaining work: capability discovery for third-party backends, backend-neutral progress events, and full Outcome JSON reporting across all commands.

Update (2025-11-20): Single-backend selection and scoped prompt profiles
- Agent selection is now a single-backend choice per command: each `[agents.<scope>]` table resolves to exactly one backend, and commands fail fast when that backend rejects the run instead of silently falling back to wire. `fallback_backend`/`fallback-backend` keys are rejected in both root and scoped config with a deprecation error so operators must fix broken backends rather than relying on hidden fallbacks (see `vizier-core/src/config.rs::FALLBACK_BACKEND_DEPRECATION_MESSAGE` and the associated tests).
- Prompt orchestration is wired through `[agents.<scope>.prompts.<kind>]` profiles in `.vizier/config.toml`, which bind prompt text (inline or via `path`) to backend/model/reasoning overrides for that scope+kind. Repo-level `.vizier/*.md` prompt files and legacy `[prompts.*]` keys remain as fallbacks, but the primary story for plan/approve/review prompts is now the agent-scoped profile. README, AGENTS.md, and `example-config.toml` document the new shape so backends and prompts can be tuned together per command without introducing parallel config surfaces.

Update (2025-11-21): Agent runner/display abstraction wired through Codex
- `vizier-core/src/agent.rs` now defines a backend-neutral `AgentRunner`/`AgentDisplayAdapter` interface plus a `FallbackDisplayAdapter` for wire events, and `AgentSettings` resolves each scope to a concrete runner/display pair (`CodexRunner` + `CodexDisplayAdapter` for `backend = "agent"`, no runner + fallback adapter for `backend = "wire"`).
- Auditor flows now call into this interface instead of hard-wiring Codex: agent-backed commands construct `AgentRequest` values (prompt, repo_root, agent runtime command/profile/bounds, extra_args, output mode, scope) and let the selected runner stream `AgentEvent` progress into the display adapter, which turns Codex JSON into `[codex] phase — message` history lines while the fallback adapter renders wire events as `[wire:<scope>]`.
- Remaining work for this thread focuses on actual pluggability beyond Codex (declaring and enforcing backend capabilities, supporting third-party runners/adapters, and making progress/telemetry fully backend-neutral) plus Outcome/session JSON that consistently reports which agent backend handled each run.

Update (2025-11-24): Gemini backend adapter + defaults
- Added `BackendKind::Gemini` with a `GeminiRunner`/`GeminiDisplayAdapter` pair that runs the `gemini` CLI in `--output-format stream-json` mode, feeds prompts on stdin, adapts JSONL events into `[gemini]` progress lines, aggregates assistant text/usage (falling back to stderr JSON when needed), and fails fast on missing binaries, non-zero exits, or empty assistant output. Passthrough still mirrors backend stdout/stderr to the CLI’s stderr while capturing the final message.
- Agent runtime normalization now defaults the command to `gemini` whenever a scope sets `backend = "gemini"` but leaves the command empty or at the Codex default; CLI help and agent-style enforcement strings now advertise `agent|gemini` as the supported backends. Agent/Gemini backends ignore CLI `--model` overrides (wire remains the only backend honoring that flag). Tests cover runner/display resolution and the default command behavior, and README/example-config show how to pin a scope to Gemini.

## Repo-local config precedence (Snapshot: Code state — repo/global configs now layer; env fallback only when no config files)

Tension
- Config now layers global defaults with repo overrides; we need the precedence to stay predictable for agent/prompt/gate settings and to ensure env-driven config only comes into play when no config files exist so operators keep reproducible defaults across machines.

Desired behavior (Product-level)
- After CLI flag resolution, Vizier loads the global config as a base (when present) and overlays `.vizier/config.toml` (or `.json`) while preserving unset defaults; env paths should only be used when no config files exist so stale overrides do not mask repo/global settings.
- Docs (README + AGENTS.md) and `example-config.toml` explain the merged precedence and show how agent/prompt/gate settings inherit from global when repo keys are absent while allowing CLI overrides to win.

Acceptance criteria
- `vizier-core/src/config.rs` exposes helpers such as `project_config_path(root: &Path)`, `global_config_path()`, and `env_config_path()` so discovery order and layering are explicit and unit-tested (TOML preferred over JSON, blank env vars ignored).
- `vizier-cli/src/main.rs` (config bootstrap) reuses the existing `project_root` to load the global layer (if present) and the repo layer, logs both, and only consults `VIZIER_CONFIG_FILE` when no config files exist.
- README.md, AGENTS.md, and `example-config.toml` document the merged order: CLI flag → global + repo layers (both when present) → `VIZIER_CONFIG_FILE` fallback only when no config files exist.
- Tests cover inheritance across layers (agent defaults, review checks, gate options) and the env fallback path when repo/global configs are missing. (`tests/src/lib.rs`, unit tests in `vizier-core/src/config.rs`)

Status
- Update (2025-11-22): Config loading now merges global defaults then overlays repo overrides when `--config-file` is absent, logs each source, and only reads `VIZIER_CONFIG_FILE` when no config files exist; docs/examples reflect the merged precedence and tests guard agent/gate/review inheritance under the layered story.
- Shipped via `.vizier/implementation-plans/we-need-to-allow-project-level-c.md` (2025-11-17) with repo/global discovery helpers; the layering/env fallback tightening landed subsequently.
