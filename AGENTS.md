Please observe `.vizier/.snapshot`, the README.md, and the various prompts in lib.rs before planning so you can get a strong feel for what this project is about.
Please also take a careful look around the implementation to understand the architectural and styling patterns before implementating

Need the draft → approve → review → merge choreography? Read `docs/workflows/draft-approve-merge.md` before editing plan branches so you understand how Vizier manages worktrees, commits, review artifacts, and merge sentinels.
Session transcripts now live under `.vizier/sessions/<session_id>/session.json`; reference those artifacts (not Git commits) when you need to audit prior conversations or reload context.

## Scoped agent configuration

Vizier ships with a pluggable agent interface. Declare defaults under `[agents.default]` and override specific commands with `[agents.ask|save|draft|approve|review|merge]`. Each table accepts the same keys as the CLI: `backend`, `fallback_backend`, `model`, `reasoning_effort`, and a nested `[agents.<scope>.codex]` table for `binary_path`, `profile`, `bounds_prompt_path`, and `extra_args`.

```toml
[agents.default]
backend = "codex"
fallback_backend = "wire"

[agents.ask]
backend = "wire"
model = "gpt-4.1"
reasoning_effort = "medium"

[agents.review.codex]
profile = "compliance"
```

Precedence is deterministic: `CLI flags → [agents.<command>] → [agents.default] → legacy top-level keys`. CLI overrides apply only to the command being executed, so `vizier ask --backend wire` leaves other commands untouched. When the resolved backend is Codex, `-p/--model` is ignored and Vizier emits a warning so operators know to adjust `[agents.<scope>]` if they want the wire stack.
