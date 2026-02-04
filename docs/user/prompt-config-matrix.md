# Prompt scopes & prompt-config matrix

This document is the canonical reference for how Vizier maps CLI commands (“scopes”) to prompt templates (“kinds”), and which configuration levers control each pairing.

- **Scopes** (commands): `ask`, `save`, `draft`, `approve`, `review`, `merge`
- **Prompt kinds**: `documentation`, `commit`, `implementation_plan`, `review`, `merge_conflict`

Only these prompt kind keys are accepted; aliases like `base`, `system`, `plan`, `refine`, and `merge` are rejected.

Use this as the single place to answer:
- “Which prompt text does this command use?”
- “Where do I put a custom template?”
- “Which knobs let me change agent or documentation behavior?”

## Scope × prompt-kind usage

The table below shows which prompt kinds each scope actually uses in the current CLI, and what they are used for at a high level.

| Scope   | `documentation`                                                                                         | `commit`                                                   | `implementation_plan`                                              | `review`                                                          | `merge_conflict`                                               |
|---------|---------------------------------------------------------------------------------------------------------|------------------------------------------------------------|--------------------------------------------------------------------|-------------------------------------------------------------------|----------------------------------------------------------------|
| `ask`   | System prompt for `vizier ask` and snapshot bootstrap (`vizier init-snapshot`)                          | Commit message template when `vizier ask` produces commits | *Not used*                                                         | *Not used*                                                        | *Not used*                                                     |
| `save`  | System prompt for `vizier save` (snapshot/narrative updates + optional code edits)                      | Commit message template when `vizier save` produces commits| *Not used*                                                         | *Not used*                                                        | *Not used*                                                     |
| `draft` | *Not used* (draft flows only use `implementation_plan`)                                                 | *Not used*                                                 | Plan template for `vizier draft` (implementation plan Markdown)    | *Not used*                                                        | *Not used*                                                     |
| `approve` | System prompt for `vizier approve` when implementing a stored plan on the draft branch                | Commit messages come from Codex summaries, not the commit prompt template | *Defined in config but not used by CLI today*                      | *Not used*                                                        | *Not used*                                                     |
| `review`| System prompt for optional “fix-up” passes after `vizier review` critiques                              | Commit messages come from Codex summaries, not the commit prompt template | *Not used*                                                         | Critique template for `vizier review` (plan vs diff vs checks)    | *Not used*                                                     |
| `merge` | System prompt for `vizier merge`’s narrative refresh step on the plan branch (`refresh_plan_branch`)    | *Not used*                                                 | *Not used*                                                         | CI/CD auto-fix flows reuse agent settings for `review` kind only for agent/docs toggles; the text template is built-in | Conflict-resolution template for `vizier merge --auto-resolve-conflicts` |

Notes:
- “System prompt” means the **documentation-style** wrapper that carries snapshot + narrative doc context (via `<snapshot>`/`<narrativeDocs>` blocks) plus `<task>`/`<instruction>` payloads.
- “Commit messages from Codex summaries” means commit bodies are synthesized from Codex’s summary output, not from the `commit` prompt template. You still configure the commit template for cases where Vizier asks the model to write a message from a diff.
- The `implementation_plan` prompt kind is only used by `vizier draft` today; `vizier approve` consumes the rendered plan document instead of re-prompting with the plan template.

## Prompt resolution & override order

For any given **scope × kind** pair, Vizier resolves prompt text in this order:

1. **Scoped agent prompt profile** (highest precedence)  
   - Table: `[agents.<scope>.prompts.<kind>]` (or `[agents.default.prompts.<kind>]`)  
   - Shape:
     - Inline text: a string value, or `{ text = "..." }`, or `{ prompt = "..." }`
     - File-based: `{ path = "relative/path.md" }` or `{ file = "relative/path.md" }`
   - Optional nested agent overrides: `[agents.<scope>.prompts.<kind>.agent]` (see below)
   - Effect:
     - Sets the exact template text for this scope+kind.
     - May attach agent/runtime/documentation overrides just for this pairing.

2. **Repo-local prompt files under `.vizier/`**  
   - Vizier looks in `.vizier/` for the first existing file per kind:
    - `documentation`: `DOCUMENTATION_PROMPT.md`
    - `commit`: `COMMIT_PROMPT.md`
    - `implementation_plan`: `IMPLEMENTATION_PLAN_PROMPT.md`
    - `review`: `REVIEW_PROMPT.md`
    - `merge_conflict`: `MERGE_CONFLICT_PROMPT.md`

3. **Baked-in defaults** (lowest precedence)  
   - Constants in `vizier-kernel/src/prompts.rs`:
    - `DOCUMENTATION_PROMPT`
    - `COMMIT_PROMPT`
    - `IMPLEMENTATION_PLAN_PROMPT`
    - `REVIEW_PROMPT`
    - `MERGE_CONFLICT_PROMPT`

Prompt text resolution only consults `[agents.<scope>.prompts.<kind>]` and `.vizier/*_PROMPT.md` files; `[prompts]` and `[prompts.<scope>]` tables are ignored, and `.vizier/BASE_SYSTEM_PROMPT.md` is not read.

In practice:
- Treat `[agents.<scope>.prompts.<kind>]` as the **primary surface** for customization.
- Use `.vizier/*.md` when you want repo-local prompt files without touching config.

## Documentation prompt toggles

Every scope can control how much narrative context is injected into documentation-style prompts via `[agents.<scope>.documentation]` (or `[agents.default.documentation]` for defaults):

```toml
[agents.default.documentation]
enabled = true                # use the documentation prompt template at all
include_snapshot = true       # include <snapshot> … </snapshot> block
include_narrative_docs = true   # include <narrativeDocs> … </narrativeDocs> block

[agents.merge.documentation]
enabled = false               # skip documentation prompt for merge-time auto-fixes
include_snapshot = false
include_narrative_docs = false
```

- When `enabled = false` for a given scope+`documentation` kind, Vizier **skips the documentation template text** but still injects the standard agent bounds and the `<task>`/`<instruction>` payload.
- `include_snapshot` and `include_narrative_docs` gate whether the snapshot and narrative docs are embedded for that scope.
- These settings also apply when documentation-style prompts are used during plan implementation, review fix-up, and merge-time narrative refresh.
- Narrative context is sourced only from `.vizier/narrative/` (snapshot, glossary, threads). `.vizier/todo_*.md` is not read.

## Agent overrides per prompt

Agent behavior for a given scope+kind can be customized in two layers:

1. **Scope-wide agent overrides**
   - Table: `[agents.<scope>]`
   - Keys:
     - `agent = "codex" | "gemini" | "<custom shim>"`
     - `[agents.<scope>.agent]` for runtime wiring (`label` / `command` plus `output` and optional `progress_filter` when wrapping JSON streams)
     - `[agents.<scope>.documentation]` for documentation prompt toggles (see above)

2. **Prompt-local agent overrides**
   - Nested under a prompt profile:
     ```toml
     [agents.review.prompts.review]
     path = "./prompts/review.md"
     agent = "codex"

     [agents.review.prompts.review.agent]
     label = "codex"                  # or set `command = [...]`
     ```

   - These overrides apply only when that **scope+kind** is in use; other prompt kinds for the same scope inherit from `[agents.<scope>]` and `[agents.default]`.

Remember:
- Each command resolves to a single agent selector; misconfigured entries cause the command to fail rather than silently falling back.

## Where to look for concrete examples

- `example-config.toml` — end-to-end examples of:
  - `[agents.default]` and per-scope `[agents.ask|save|draft|approve|review|merge]`
  - `[agents.<scope>.prompts.<kind>]` for documentation, implementation-plan, and review prompts
  - `[agents.<scope>.documentation]` toggles for merge-time conflict resolution
- `.vizier/config.toml` — repo-local overrides that travel with Git history.
- `README.md` — high-level configuration overview and how prompt profiles fit into the draft → approve → review → merge workflow.
- `docs/user/workflows/draft-approve-merge.md` — workflow-level behavior; this file is the authoritative matrix for prompt scopes and kinds.

When adding new commands or prompt kinds, update **this matrix first**, then refresh the references in `README.md`, `AGENTS.md`, the workflow docs, and `example-config.toml` so downstream agents and humans can rely on a single, consistent story.
