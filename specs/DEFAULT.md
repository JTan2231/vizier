# Feature Spec: Global Default Workflows (`DEFAULT`)

## Status

- Proposed implementation spec.
- Scope date: 2026-02-15.
- This spec defines the default/global workflow posture for `vizier run`.

## Purpose

Provide a project-independent place for reusable workflow templates while keeping execution repo-bound and predictable.

The goals are:

1. A dedicated global `workflows/` folder whose templates are runnable by alias.
2. Default-on behavior (no manual opt-in required).
3. Install-time seeding of baseline stage templates (`draft`, `approve`, `merge`).

## Decision

Vizier MUST support a global workflow directory and treat each workflow file in that directory as an implicit `vizier run <flow>` target.

This global workflow discovery is enabled by default.

`install.sh` MUST install the canonical stage templates into the global workflow directory.

## Scope

In scope:

- `vizier run` flow-source resolution.
- Config schema/defaults for global workflow discovery.
- Install-script behavior for baseline templates.
- Runtime path-safety policy updates to allow approved non-repo workflow files.

Out of scope:

- Reintroducing top-level `vizier draft|approve|merge` wrappers.
- Allowing Vizier operation outside a git repository.
- Changing scheduler/job semantics.

## Global Workflow Directory

### Default Location

Global workflows live under:

- `<base_config_dir>/vizier/workflows`

Where `<base_config_dir>` follows existing config directory discovery:

1. `VIZIER_CONFIG_DIR`
2. `XDG_CONFIG_HOME`
3. `APPDATA`
4. `HOME/.config`
5. `USERPROFILE/AppData/Roaming`

### Supported Files

Discovery includes readable files ending in:

- `.toml`
- `.json`

Implicit alias name is the filename stem (for example `draft.toml` -> `draft`).

## Config Contract

Add a workflow-global section:

```toml
[workflow.global_workflows]
enabled = true   # default
dir = ""         # optional override; empty means use default location
```

Rules:

1. `enabled` defaults to `true`.
2. Operators can disable with `enabled = false`.
3. `dir`, when set, points to the global workflow directory and overrides the default location.

## Flow Resolution Order

`vizier run <flow>` resolves sources in this precedence order:

1. Explicit file source (`file:<path>`, direct path, `.toml`/`.json` path-like input).
2. Configured alias from merged `[commands]` (repo overrides global via existing config layering).
3. Selector identity lookup (`id@version`) among repo-local template files.
4. Repo fallback files (`.vizier/<flow>.toml|json`, `.vizier/workflows/<flow>.toml|json`).
5. Global-workflow implicit alias (`<global_workflow_dir>/<flow>.toml|json`) when `workflow.global_workflows.enabled = true`.
6. Error.

Repo-local sources remain authoritative over global implicit aliases.

Repo-level canonical workflow directory is `.vizier/workflows` (plural).

## Path Safety Policy

Current `vizier run` behavior rejects template files outside the repository root.

This spec updates that policy:

1. Keep rejecting arbitrary out-of-repo paths by default.
2. Allow out-of-repo paths only when they are under the resolved global workflow directory.
3. Continue canonicalization checks to prevent traversal/symlink escapes.

## Install Contract

`install.sh` MUST seed canonical stage templates from:

- `.vizier/workflows/draft.toml`
- `.vizier/workflows/approve.toml`
- `.vizier/workflows/merge.toml`

Into the global workflow directory.

### Install Variables

Add:

- `WORKFLOWSDIR` (default: resolved global workflow directory)

Behavior:

1. Install creates `WORKFLOWSDIR` when missing.
2. Install records copied workflow files in the manifest for uninstall parity.
3. Existing user files at destination SHOULD be preserved by default (skip + report), not silently overwritten.

## UX Contract

Examples:

- `vizier run draft` works in any git repo when `WORKFLOWSDIR/draft.toml` exists and no higher-precedence repo source overrides it.
- `vizier run my-flow` resolves `WORKFLOWSDIR/my-flow.toml` (or `.json`) as an implicit alias.

`vizier` remains git-repo-scoped. Global workflows do not make execution repo-independent.

## Acceptance Criteria

1. Default config resolves global workflows with `enabled = true` and no explicit setup.
2. `vizier run draft` resolves global `draft.toml` when repo-local alias/template is absent.
3. Repo-local alias/template with the same flow name overrides global implicit alias.
4. Setting `[workflow.global_workflows].enabled = false` disables implicit global workflow discovery.
5. `vizier run` allows templates under the configured global workflow directory while still rejecting other out-of-repo paths.
6. `install.sh` installs `draft/approve/merge` templates into `WORKFLOWSDIR` and records them in uninstall manifest.

## Testing Requirements

Coverage MUST include:

1. Config defaults and override parsing for `[workflow.global_workflows]`.
2. Resolver precedence tests for repo-vs-global workflow alias collisions.
3. Path-safety tests validating allowlist behavior for global workflow directory and rejection outside allowlist.
4. Install-script tests for workflow template install/uninstall manifest behavior.

## References

- `vizier-core/src/config/load.rs`
- `vizier-kernel/src/config/defaults.rs`
- `vizier-cli/src/workflow_templates.rs`
- `install.sh`
- `docs/user/installation.md`
- `docs/user/config-reference.md`
- `docs/user/workflows/stage-execution.md`
- `specs/PRIMITIVE_STAGE_TEMPLATES.md`
