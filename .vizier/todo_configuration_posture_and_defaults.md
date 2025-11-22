# Config-first posture and defaults

Thread: Configuration posture + defaults (cross: Agent workflow orchestration, Agent backend abstraction + pluggable CLI agents, Architecture doc gate + compliance, Stdout/stderr contract + verbosity)

Snapshot anchor
- Narrative theme — Config-first posture (Running Snapshot — updated).
- Code state — Repo/global configs now layer: global defaults load first, repo `.vizier/config.*` overlays, and env `VIZIER_CONFIG_FILE` only applies when no config files exist.

Tension
- As Vizier evolves as a layer above Git and external agents, new features can feel over-opinionated or opaque when they hard-code behaviors instead of offering configuration levers.
- Existing configuration stories (agent backends, CI/CD gate script, repo-local config precedence) are landing piecemeal, so operators lack a cohesive mental model for where to set defaults and how CLI flags interact with repo and global config.

Desired behavior (Product-level)
- Treat configuration as a first-class surface: every new feature that changes workflow, IO, or agent behavior ships with a documented config entry and a clear flag override, plus a sensible default that works out of the box.
- Keep the configuration story small and coherent by grouping related knobs (agents, gates, prompts, IO/mode, snapshot/TODO posture) so operators can reason about them without scanning multiple scattered docs.
- Defaults deliver high utility with low surprise: a fresh repo using `.vizier/config.toml` and the provided examples should get a safe, non-intrusive experience, while power users can tighten or relax gates without patching code.
- Future threads (stdout/stderr contract, Outcome summaries, architecture doc gate, agent orchestration, pluggable backends) integrate with this configuration story instead of inventing parallel toggles or one-off environment variables.

Acceptance criteria
- Docs include an operator-facing configuration guide that explains the main configuration groups (agents, gates, workflows, IO/mode, snapshot/TODO posture) and their precedence (CLI vs repo vs global), referencing `.vizier/config.toml` and `example-config.toml`.
- New CLI features and narrative threads explicitly route their knobs through this guide: flags map to config keys, and repositories can opt in/out or adjust behavior without code changes.
- Help output and config examples stay in sync: for at least one representative feature in each group (agent backend, CI/CD gate, verbosity/mode, workflow gating), there is a round-trip example showing “set in config → override on CLI” that behaves as documented.
- Tests cover at least: default behavior with no config present, behavior when repo-level config is present, and CLI-override precedence for representative knobs (for example, agent backend, gate script, mode), asserting that behavior matches the documented configuration story.

Pointers
- `.vizier/config.toml` and `example-config.toml` for current gate/agent defaults and precedence.
- Agent backend abstraction and repo-local config precedence sections in `.vizier/todo_pluggable_agent_backends.md`.
- CI/CD gate thread in `.vizier/todo_agent_command_cicd_gate.md`.
- Stdout/stderr contract + verbosity TODO in `.vizier/todo_stdout_stderr_contract_and_verbosity`.
- README, AGENTS.md, and workflow docs that describe `[agents.*]`, `[merge.cicd_gate]`, and the draft → approve → review → merge flags.

Update (2025-11-22): Config loading now layers global defaults with repo overrides when `--config-file` is absent, logs both sources, and only consults `VIZIER_CONFIG_FILE` when no config files exist; docs/examples/tests cover agent/gate/review inheritance under the merged precedence.
