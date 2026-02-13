# Prompt aliases & prompt-config matrix

This document is the canonical reference for how Vizier maps command aliases to prompt kinds, and which config tables control each pairing.

- Command aliases: `save`, `draft`, `approve`, `review`, `merge`, `patch`, `build_execute`
- Prompt kinds: `documentation`, `commit`, `implementation_plan`, `review`, `merge_conflict`
- Internal fallback profile: `default` (not a CLI command)

Only these prompt kinds are accepted; aliases like `base`, `system`, `plan`, `refine`, and `merge` are rejected.

## Alias x prompt-kind usage

| Command alias | `documentation` | `commit` | `implementation_plan` | `review` | `merge_conflict` |
| --- | --- | --- | --- | --- | --- |
| `save` | System prompt for `vizier save` | Commit-message template for save-generated commits | Not used | Not used | Not used |
| `draft` | Not used | Not used | Plan template for `vizier draft` | Not used | Not used |
| `approve` | System prompt for `vizier approve` plan-implementation runs | Not used by normal approve flow | Defined in config, not used directly by CLI | Not used | Not used |
| `review` | System prompt for optional review fix-up runs | Not used by normal review flow | Not used | Critique template for `vizier review` | Not used |
| `merge` | System prompt for merge-time narrative refresh | Not used | Not used | Merge CI/CD auto-fix runs reuse `review`-kind agent/docs toggles | Conflict-resolution template for `vizier merge --auto-resolve-conflicts` |
| `patch` | Inherits from the workflow nodes it queues (approve/review/merge) | Inherits node behavior | Inherits node behavior | Inherits node behavior | Inherits node behavior |
| `build_execute` | Inherits from the workflow nodes it queues | Inherits node behavior | Inherits node behavior | Inherits node behavior | Inherits node behavior |

Notes:
- `patch` and `build_execute` are orchestration aliases. Prompt/runtime behavior is typically determined by each queued workflow node capability/alias (for example `approve`, `review`, `merge`).
- Legacy command scopes still exist as a compatibility bridge (`save|draft|approve|review|merge`), but command aliases + template selectors are now the primary resolution path.

## Prompt resolution order

For a given alias + kind, prompt text resolves in this order:

1. `[agents.templates."<template-selector>".prompts.<kind>]`
2. `[agents.commands.<alias>.prompts.<kind>]`
3. Legacy `[agents.<scope>.prompts.<kind>]` (compatibility only)
4. `[agents.default.prompts.<kind>]`
5. Repo prompt files under `.vizier/`:
   - `documentation`: `DOCUMENTATION_PROMPT.md`
   - `commit`: `COMMIT_PROMPT.md`
   - `implementation_plan`: `IMPLEMENTATION_PLAN_PROMPT.md`
   - `review`: `REVIEW_PROMPT.md`
   - `merge_conflict`: `MERGE_CONFLICT_PROMPT.md`
6. Baked-in defaults in `vizier-kernel/src/prompts.rs`

`[prompts]` and `[prompts.<scope>]` are ignored, and `.vizier/BASE_SYSTEM_PROMPT.md` is not read.

## Agent/runtime override order

Selector/runtime/documentation settings resolve in this order:

1. `[agents.default]`
2. Legacy `[agents.<scope>]` (compatibility bridge)
3. `[agents.commands.<alias>]`
4. `[agents.templates."<template-selector>"]`
5. Prompt-local nested override (`[...prompts.<kind>.agent]`)
6. CLI selector override (`--agent`)

Template tables win over alias tables; alias tables win over legacy scope tables.

## Documentation prompt toggles

Documentation toggle tables:

- `[agents.default.documentation]`
- `[agents.commands.<alias>.documentation]`
- `[agents.templates."<template-selector>".documentation]`
- Legacy `[agents.<scope>.documentation]` (compatibility)

Supported keys:

- `enabled` (default `true`)
- `include_snapshot` (default `true`)
- `include_narrative_docs` (default `true`)

When `enabled = false`, Vizier skips documentation template text for that identity but still sends the task/instruction payload to the agent.

## Examples

Alias-scoped prompt text:

```toml
[agents.commands.review.prompts.review]
path = "./prompts/review.md"
agent = "codex"

[agents.commands.review.prompts.review.agent]
command = ["./examples/agents/codex/agent.sh"]
```

Template-scoped override (wins over alias):

```toml
[agents.templates."template.review@v2".prompts.review]
text = "Use stricter release-readiness criteria."

[agents.templates."template.review@v2".documentation]
enabled = false
include_snapshot = false
include_narrative_docs = false
```

Wrapper-to-template binding:

```toml
[commands]
review = "template.review@v2"
```

Legacy compatibility fallback (still accepted during migration):

```toml
[agents.review]
agent = "gemini"
```

## Validation and inspection

- `vizier plan --json` shows resolved per-command alias settings, including template selector and compatibility scope fallback.
- `vizier test-display --command <alias>` resolves settings through alias/template identities.
- Hidden/deprecated `vizier test-display --scope ...` still maps to compatibility aliases during migration.

When adding new prompt behavior, update this document first, then align `docs/user/config-reference.md` and `example-config.toml`.
