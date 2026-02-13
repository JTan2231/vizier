# Config-first posture and defaults

Thread: Configuration posture + defaults (cross: Agent workflow orchestration, Agent backend abstraction + pluggable CLI agents, Architecture doc gate + compliance, Stdout/stderr contract + verbosity)

Snapshot anchor
- Narrative theme — Config-first posture (Running Snapshot — updated).
- Code state — Repo/global configs now layer: global defaults load first, repo `.vizier/config.*` overlays, and env `VIZIER_CONFIG_FILE` only applies when no config files exist.

Tension
- As Vizier evolves as a layer above Git and external agents, new features can feel over-opinionated or opaque when they hard-code behaviors instead of offering configuration levers.
- Existing configuration stories (agent backends, CI/CD gate script, repo-local config precedence) are landing piecemeal, so operators lack a cohesive mental model for where to set defaults and how CLI flags interact with repo and global config.
- Documentation path guidance is currently split: AGENTS.md/README point to root aliases (`docs/config-reference.md`, `docs/prompt-config-matrix.md`) while canonical docs live under `docs/user/*`, creating orientation confusion and dead-link risk.

Desired behavior (Product-level)
- Treat configuration as a first-class surface: every new feature that changes workflow, IO, or agent behavior ships with a documented config entry and a clear flag override, plus a sensible default that works out of the box.
- Keep the configuration story small and coherent by grouping related knobs (agents, gates, prompts, IO/mode, snapshot/narrative posture) so operators can reason about them without scanning multiple scattered docs.
- Defaults deliver high utility with low surprise: a fresh repo using `.vizier/config.toml` and the provided examples should get a safe, non-intrusive experience, while power users can tighten or relax gates without patching code.
- Future threads (stdout/stderr contract, Outcome summaries, architecture doc gate, agent orchestration, pluggable backends) integrate with this configuration story instead of inventing parallel toggles or one-off environment variables.
- Documentation pointers remain coherent: AGENTS.md/README/workflow guidance either points directly to `docs/user/*` or explicitly labels and ships root aliases that map to the canonical files, so no guidance path is dead.

Acceptance criteria
- Docs include an operator-facing configuration guide that explains the main configuration groups (agents, gates, workflows, IO/mode, snapshot/narrative posture) and their precedence (CLI vs repo vs global), referencing `.vizier/config.toml` and `example-config.toml`.
- New CLI features and narrative threads explicitly route their knobs through this guide: flags map to config keys, and repositories can opt in/out or adjust behavior without code changes.
- Help output and config examples stay in sync: for at least one representative feature in each group (agent backend, CI/CD gate, verbosity/mode, workflow gating), there is a round-trip example showing “set in config → override on CLI” that behaves as documented.
- Tests cover at least: default behavior with no config present, behavior when repo-level config is present, and CLI-override precedence for representative knobs (for example, agent backend, gate script, mode), asserting that behavior matches the documented configuration story.
- AGENTS.md/README/workflow references for `config-reference`, `prompt-config-matrix`, and draft→approve workflow docs resolve to existing files (either direct `docs/user/*` links or intentionally provided root aliases), and snapshot/glossary orientation notes distinguish canonical locations from shorthand aliases when both are present.

Pointers
- `.vizier/config.toml` and `example-config.toml` for current gate/agent defaults and precedence.
- Agent backend abstraction and repo-local config precedence sections in `.vizier/narrative/threads/pluggable_agent_backends.md`.
- CI/CD gate thread in `.vizier/narrative/threads/agent_command_cicd_gate.md`.
- Stdout/stderr contract + verbosity thread in `.vizier/narrative/threads/stdout_stderr_contract_and_verbosity.md`.
- README, AGENTS.md, and workflow docs that describe `[agents.*]`, `[merge.cicd_gate]`, and the draft → approve → review → merge flags.

Update (2025-11-22): Config loading now layers global defaults with repo overrides when `--config-file` is absent, logs both sources, and only consults `VIZIER_CONFIG_FILE` when no config files exist; docs/examples/tests cover agent/gate/review inheritance under the merged precedence.
Update (2025-11-29): `docs/user/config-reference.md` now carries quick-start override examples (pin review to Gemini, tighten merge gate retries, disable auto-commit, per-scope prompt swaps) plus a config-vs-CLI override matrix, and AGENTS.md points to it. Added automated checks for help output (quiet + `--no-ansi`, pager suppression in non-TTY) alongside a regression test that bundled progress filters attach to any agent label with a sibling `filter.sh`.
Update (2026-02-06): Captured docs-path drift discovered during orientation checks: AGENTS.md/README currently reference root `docs/config-reference.md` and `docs/prompt-config-matrix.md` while the on-disk docs remain under `docs/user/*` (and workflow docs under `docs/user/workflows/*`). Snapshot/glossary now call out the alias mapping, and this thread tracks reconciling references versus aliases so operators do not hit dead links.
Update (2026-02-12): Re-validated docs-path drift in this worktree: `README.md` and `AGENTS.md` still point to root `docs/config-reference.md` and `docs/prompt-config-matrix.md` (with AGENTS phrasing them as canonical), while those root files are absent and only `docs/user/*` exists on disk. This keeps the dead-link risk active until aliases or link targets are reconciled.
Update (2026-02-13): Global CLI surface cleanup landed for config/output posture. Root globals now keep only meaningful runtime controls (`--agent`, `--follow`, verbosity/ANSI/session/config/push/no-commit), while stale globals (`--background`, `--no-background`, global `--json`, `--pager`, `--agent-label`, `--agent-command`) were removed with hard-error migration guidance. Command-local machine output now uses `vizier plan --json` and `--format json` on list/jobs surfaces, and docs/man/tests were updated to match.
Update (2026-02-13, run aliases): Config/docs now treat `[commands]` as both the built-in wrapper selector map and the custom alias surface consumed by `vizier run <alias>`. `docs/user/config-reference.md` and `docs/user/workflows/draft-approve-merge.md` now document composed repo-local aliases (for example `develop`) and the runtime precedence (`[commands.<alias>]` first, then repo fallback files) so operators can adopt one-command workflows without adding new global CLI flags.
