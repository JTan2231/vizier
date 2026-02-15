# MIGRATE: Hard-Cut Compatibility Removal Spec

## Purpose

Define a single migration plan to remove all legacy/compatibility bridges and run Vizier in canonical-only mode.

This document specifies:
- what compatibility surfaces are removed,
- what breaks when they are removed,
- what data/config/content must be migrated first,
- how to roll out safely.

## Goals

- Remove legacy runtime bridges and fallback resolution paths.
- Keep canonical surfaces only.
- Make failures explicit and deterministic when legacy shapes are encountered.
- Provide a migration sequence that can be run once per repository/worktree.

## Non-Goals

- Preserving execution of legacy artifacts without migration.
- Maintaining dual-write behavior for historical keys indefinitely.
- Supporting old documentation alias files after migration completes.

## Canonical-Only Target State

- Workflow `uses` IDs: only `cap.env.*`, `cap.agent.invoke`, and `control.*`.
- Template selector format: `template.name@vN`.
- Stage workflow location: `.vizier/workflows/*.toml` only.
- Command mapping source: `[commands]` and explicit `file:`/path selectors.
- Agent overrides: `[agents.default]`, `[agents.commands.<alias>]`, `[agents.templates."<selector>"]`.
- Job metadata identity: `command_alias`, `workflow_template_selector`, `workflow_executor_*`, `workflow_control_policy`, `execution_root`.
- Snapshot path: `.vizier/narrative/snapshot.md`.
- Agent shim layout: `<shim_root>/<label>/agent.sh` and optional `<shim_root>/<label>/filter.sh`.

## Compatibility Bridges To Remove

1. Workflow path fallback bridge
- Remove support for `.vizier/workflow/*.toml` and `.vizier/workflow/*.json` fallback resolution.
- Keep `.vizier/workflows/*` only.

2. Legacy selector identity bridge
- Remove dotted selector parsing (`template.name.vN`).
- Keep only canonical `template.name@vN`.

3. Legacy workflow template config bridge
- Remove fallback from alias lookup into `[workflow.templates]`.
- Resolve aliases from `[commands]` only.

4. Legacy agent scope bridge
- Remove `[agents.<scope>]` compatibility (`save`, `draft`, `approve`, `review`, `merge`, plus bridge mappings for `patch`, `build_execute`).
- Keep `[agents.commands.<alias>]` and `[agents.templates."<selector>"]`.

5. Legacy job metadata and artifact bridges
- Remove `metadata.scope` fallback for runtime settings resolution.
- Remove `metadata.worktree_path` fallback when resolving execution root.
- Remove deserialize-only artifact alias `ask_save_patch`.
- Remove legacy file acceptance/cleanup for `ask-save.patch` and `save-input.patch`.
- Remove `workflow_capability_id` historical metadata handling.

6. Legacy snapshot alias bridge
- Remove `.vizier/.snapshot` discovery/acceptance.

7. Legacy shim filename bridge
- Remove shim fallback names `<label>.sh` and `<label>-filter.sh`.

8. Root docs alias bridge
- Remove `docs/config-reference.md` and `docs/prompt-config-matrix.md` compatibility alias files once all references are repointed.

## What Breaks Without Migration

1. `vizier run draft|approve|merge` can fail if aliases still point at `.vizier/workflow/*`.
2. Alias resolution can fail for configs still using dotted selectors (`template.*.vN`).
3. Repositories using only `[workflow.templates]` without `[commands]` alias entries lose alias resolution.
4. Repositories using `[agents.<scope>]` lose scoped agent/prompt overrides.
5. Historical job records can stop replaying/retrying/showing correctly if they still rely on:
- `ask_save_patch`,
- `ask-save.patch` / `save-input.patch`,
- `metadata.scope`,
- `metadata.worktree_path` without `execution_root`.
6. Repositories still using `.vizier/.snapshot` stop loading snapshot context.
7. Custom/older shim installations using flat filenames stop resolving.
8. Orientation docs/man refs break if root alias docs are removed before link updates.

## Required Migrations

1. Workflow files and alias pointers
- Move/canonicalize all stage templates to `.vizier/workflows/*.toml`.
- Update `[commands]` entries to `file:.vizier/workflows/<flow>.toml`.
- Remove `.vizier/workflow/` files once no references remain.

2. Selector format normalization
- Rewrite all dotted selectors:
  - `template.foo.v1` -> `template.foo@v1`.
- Apply in repo config, tests, docs, fixtures, and defaults.

3. Agent config normalization
- Rewrite legacy scope tables:
  - `[agents.save]` -> `[agents.commands.save]` (or template-scoped profile where appropriate).
- Move prompt overrides under canonical scopes only.

4. Persisted job artifact/metadata normalization
- For retained `.vizier/jobs/*` records:
  - map `scope` -> `command_alias` (if missing),
  - map `worktree_path` -> `execution_root` (if missing),
  - map `ask_save_patch` artifact kind -> `command_patch`,
  - rename `ask-save.patch` -> `command.patch` where present,
  - delete stale `save-input.patch`.
- If historical jobs are not needed, clear `.vizier/jobs/` instead of transforming.

5. Snapshot normalization
- Move `.vizier/.snapshot` to `.vizier/narrative/snapshot.md`.
- Ensure glossary remains at `.vizier/narrative/glossary.md`.

6. Shim normalization
- Ensure bundled/custom shims use directory layout:
  - `<root>/<label>/agent.sh`
  - `<root>/<label>/filter.sh` (optional)
- Update config overrides to explicit command paths where needed.

7. Docs/man link normalization
- Repoint all refs to canonical docs under `docs/user/*`.
- Remove root alias docs only after references are updated.

## Repo-Local Immediate Migration Needed

Current `.vizier/config.toml` maps stage aliases to legacy paths:
- `draft = "file:.vizier/workflow/draft.toml"`
- `approve = "file:.vizier/workflow/approve.toml"`
- `merge = "file:.vizier/workflow/merge.toml"`

Immediate update required:
- switch these to `.vizier/workflows/*.toml`.

## Rollout Plan

1. Phase 0: Inventory and fail-fast scaffolding
- Add strict validation errors for each soon-to-be-removed bridge behind temporary guard flags.
- Add migration diagnostics that point to canonical replacements.

2. Phase 1: In-repo migration commit
- Rewrite configs, workflow paths, selectors, and docs to canonical forms.
- Transform or purge persisted job artifacts.
- Update tests/fixtures to canonical-only assumptions.

3. Phase 2: Hard cut
- Remove bridge code and legacy aliases.
- Remove dual-write and fallback logic.
- Keep only canonical parsing/dispatch.

4. Phase 3: Post-cut cleanup
- Delete compatibility docs language and alias files.
- Remove obsolete migration helpers/tests.

## Validation and Acceptance

1. Functional acceptance
- `vizier run <alias>` resolves only through canonical paths/selectors.
- Legacy inputs fail with explicit canonical migration errors.
- Jobs show/retry/attach works for canonical records.

2. Migration acceptance
- Existing repo configs are migrated with no legacy keys/paths remaining.
- No `.vizier/workflow/*` references remain in code/docs/tests/config.
- No dotted selector strings (`template.*.vN`) remain.

3. Regression acceptance
- `./cicd.sh` passes.
- Updated integration tests cover:
  - canonical-only selector parsing,
  - canonical workflow path resolution,
  - rejection of legacy agent scopes,
  - rejection of legacy artifact kinds/metadata fallback,
  - snapshot canonical path only.

## Test Plan Updates Required

1. Remove or rewrite tests that assert compatibility behavior:
- legacy selector resolution tests,
- legacy scope fallback tests,
- legacy artifact alias deserialize tests,
- retry cleanup assertions around `ask-save.patch`/`save-input.patch`,
- legacy snapshot fallback tests.

2. Add strict negative tests:
- dotted selector rejection,
- `.vizier/workflow/*` resolution rejection,
- `[agents.<scope>]` rejection,
- `ask_save_patch` rejection,
- missing `execution_root` rejection when only legacy metadata exists.

## Operational Guidance

- If preserving historical job artifacts is not required, the lowest-risk migration is:
  - migrate config/workflow/docs,
  - clear `.vizier/jobs/`,
  - then cut bridges.
- If history must be preserved, run an explicit one-time transformation for job records/artifacts before bridge removal.

## Completion Criteria

Migration is complete when:
- no compatibility bridges listed here remain in runtime code,
- no legacy-form config/path/selector appears in repo content,
- all tests and docs represent canonical-only behavior,
- CI (`./cicd.sh`) is green.
