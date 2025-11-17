---
plan: we-need-to-allow-project-level-c
branch: draft/we-need-to-allow-project-level-c
status: implemented
created_at: 2025-11-17T17:15:28Z
spec_source: inline
implemented_at: 2025-11-17T17:19:58Z
---

## Operator Spec
we need to allow project-level configs (something like .vizier/config.toml). this should sit between the config file flag (highest precedence) and the VIZIER_CONFIG_FILE env var (lowest precedence).

## Implementation Plan
## Overview
Project operators want repo-local defaults that travel with `.vizier/` while still respecting existing CLI/environment overrides. Today Vizier only reads `--config-file` or global locations derived from `VIZIER_CONFIG_FILE`/XDG, so per-project agent settings or gates must be injected manually. We will add support for `.vizier/config.{toml,json}` inside each repo, making it the default configuration source whenever no CLI flag is provided while keeping the CLI override highest and the `VIZIER_CONFIG_FILE` env override lowest. This reduces friction for the pluggable-agent and workflow orchestration threads by letting repositories codify their preferred `[agents.*]`, gates, and prompt overrides directly in-tree.

## Execution Plan
1. **Define the new precedence contract inside `vizier-core`**
   - Add helpers such as `Config::project_config_path(root: &Path)` and `Config::global_config_path()` so we can distinguish repo-local vs user-global config paths.
   - Update `default_config_path()` (or introduce a new function) so that it no longer hard-codes the env var lookup; instead, have a dedicated `env_config_path()` that reads `VIZIER_CONFIG_FILE`.
   - Document the canonical search order in code comments: `--config-file flag` → `.vizier/config.toml` (fallback to `.json`) → global config under `~/.config/vizier/config.toml` (or platform equivalents) → `VIZIER_CONFIG_FILE`.
   - Ensure path detection gracefully handles missing `.vizier` directories and supports both TOML and JSON, reusing `Config::from_path` to sniff extensions.

2. **Teach the CLI to load repo-local configs before falling back**
   - In `vizier-cli/src/main.rs`, reuse the already-detected `project_root` to probe `.vizier/config.toml` / `.vizier/config.json`. Only attempt this when the CLI flag isn’t set.
   - When a repo config exists, load it; otherwise fall through to the existing global/default logic, and only then consult `VIZIER_CONFIG_FILE`.
   - Preserve current error handling (surfacing the path that failed to parse) so operators immediately know which file broke.
   - Emit a debug/info line (respecting verbosity settings) when a repo config is picked up to aid troubleshooting across agent backends.

3. **Update docs and samples so operators know about repo configs**
   - Expand the README “Configure via CLI flags or config file” section to describe the new `.vizier/config.{toml,json}` option and the precedence stack (flag → repo file → global → env).
   - Update AGENTS.md (or whichever doc enumerates config knobs) with the same ordering so agents know where to place repo-specific `[agents.*]` blocks.
   - If needed, drop a short comment in `example-config.toml` hinting that it can be copied into `.vizier/config.toml`, keeping that file authoritative for supported keys.

4. **Add tests that lock in the precedence**
   - Extend the integration harness in `tests/src/lib.rs`: create a `.vizier/config.toml` with distinctive `[agents.default]` overrides, set `VIZIER_CONFIG_FILE` to a conflicting config, and assert that running a command (e.g., `vizier list` or `vizier ask` dry run) resolves the repo-local backend/model.
   - Add a complementary test showing that when `.vizier/config.*` is absent, the env var path is honored.
   - Unit-test the new path helpers (e.g., repo helper prefers `.toml` over `.json`, env helper trims blanks) to guard against regressions in future refactors.

## Risks & Unknowns
- **File discovery ambiguity**: Some repos might already stash other artifacts in `.vizier/`; we must ensure only `config.toml`/`config.json` are considered to avoid surprising loads. Mitigation: explicit filename matching plus clear errors.
- **Parsing failures halting CLI startup**: A malformed repo config would now block the CLI even if a working global config exists. Mitigation: emphasize in docs/tests that parse errors surface the exact file and operators can delete/rename it.
- **Env-var compatibility**: Inverting precedence might surprise operators who relied on `VIZIER_CONFIG_FILE` overriding everything. We should call out the new order in docs/release notes so they migrate to repo configs or CLI flags.

## Testing & Verification
- Integration tests covering (a) repo config winning over the env var and (b) env var being used when no repo config exists.
- Unit tests for the new helper functions (path discovery, env trimming) to keep behavior deterministic across platforms.
- Manual sanity check: drop an `.vizier/config.toml` that sets `backend = "wire"`, run `vizier ask --help` (or another benign command), and confirm the resolved backend reported in session logs/verbosity output matches the repo config even when `VIZIER_CONFIG_FILE` points elsewhere.

## Notes
- Once this is in place, future work (architecture-doc gates, per-repo agent profiles) can assume a first-class config file lives alongside `.vizier/.snapshot`, reducing reliance on global state.
