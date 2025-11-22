---
plan: agent-resolve
branch: draft/agent-resolve
---

## Operator Spec
as we start adding more agents to be natively supported, we should start resolving the agent binary default if none other is specified instead of forcing codex as the default. for now, lets only support resolution for codex and gemini. this is primarily for resolving the runtime option of the startup command as exec won't be universal, and we want to be able to find relevant binaries without the user having to provide them manually. see ~/ts/gemini-cli for the gemini cli source code

## Implementation Plan
## Overview
- Add automatic agent binary resolution so agent-backed commands no longer assume `codex exec` when no runtime command is configured, aligning with the pluggable-agent posture and reducing setup friction.
- Support discovery for Codex and Gemini CLIs first, choosing an appropriate default startup command per agent instead of forcing `exec`.
- Ensure configuration/CLI overrides remain authoritative while autodiscovery provides sensible defaults and clear failures when no supported agent is found.

## Execution Plan
- **Define resolution strategy**: Specify a deterministic resolution order for agent runtime commands (CLI overrides → scoped config overrides → repo/global config → autodiscovery). Document which binaries/subcommands are tried for Codex vs Gemini and how ties are broken (e.g., prefer explicit backend selection, otherwise first supported binary found).
- **Extend backend identification**: Introduce a way to express the target agent flavor (Codex vs Gemini) in config/CLI resolution (e.g., backend enum or agent-kind field) so runner/adapter selection and default commands match the chosen agent. Preserve existing `agent`/`wire` semantics for backward compatibility.
- **Autodiscovery implementation**: Add a resolver that inspects PATH (and any repo-local hints) to locate supported binaries and compute the default command vector for each (handling non-`exec` entrypoints). Integrate this resolver into `Config::default`/`resolve_agent_settings` so `AgentRuntimeOptions.command` is populated lazily when empty.
- **Runner/adapter wiring**: Wire the resolved agent kind to the appropriate `AgentRunner`/`AgentDisplayAdapter` (Codex existing; add Gemini runner/adapter if event shape differs). Ensure errors for unsupported capability requests remain clear.
- **CLI integration**: Update `vizier` entrypoint to display which agent binary was resolved, honor `--agent-bin`/profile/bounds overrides, and emit actionable errors when no supported agent is discoverable. Keep session logs/outcome metadata reflecting the resolved backend/binary.
- **Docs/examples alignment**: Refresh `README.md`, `AGENTS.md`, `example-config.toml`, and any agent-related docs to describe the new discovery behavior, supported agents, override precedence, and fallback/error stories.

## Risks & Unknowns
- Gemini CLI shape and event format may diverge from Codex; may require a bespoke runner/adapter. Need to inspect the referenced `~/ts/gemini-cli` to confirm invocation and output contracts.
- Changing defaults could surprise environments relying on implicit Codex; must ensure compatibility or clear messaging when resolution picks a different agent or fails.
- PATH-based discovery must avoid picking unrelated binaries with conflicting names; may need validation probes.
- Outcome/session logging fields may need expansion to carry agent-kind/binary details without breaking existing consumers.

## Testing & Verification
- Unit tests for runtime resolution: empty command + PATH containing Codex → resolves to Codex default; only Gemini available → resolves to Gemini default; no supported binary → clear error.
- Config/CLI precedence tests: explicit `agent.command` or `--agent-bin` must override discovery; per-scope overrides respected.
- Runner/adapter selection tests: correct runner bound for each agent kind; failures when requesting unsupported capabilities.
- Integration tests for a simple agent-backed command (mocked runners if needed) verifying resolved command in session logs/outcome metadata and honoring verbosity/quiet flags.
- Documentation sanity checks: help output and example configs match the new resolution story.

## Notes
- Narrative change: default agent selection becomes “discover Codex or Gemini” instead of hard-coding Codex, advancing the pluggable-agent thread while keeping config-first precedence.
