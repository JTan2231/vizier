Please observe `.vizier/.snapshot`, the README.md, and the various prompts in lib.rs before planning so you can get a strong feel for what this project is about.
Please also take a careful look around the implementation to understand the architectural and styling patterns before implementating

Agent selection is a single-backend choice per command. Vizier no longer falls back to wire when the configured backend fails; the command aborts with the backend error so you can fix the root cause before retrying.

Need the draft → approve → review → merge choreography? Read `docs/workflows/draft-approve-merge.md` before editing plan branches so you understand how Vizier manages worktrees, commits, review artifacts, and merge sentinels.

Need to hold agent-applied changes for manual review? Pass the global `--no-commit` flag (or set `[workflow] no_commit_default = true` in `.vizier/config.toml`). Draft/approve/review will keep their temporary worktrees dirty so you can inspect and commit manually. `vizier merge` still requires an actual merge commit, so finalize the draft branch before running it.

Need predictable merge history? `vizier merge` now squashes the plan’s implementation edits into a single code commit before emitting the merge commit so every plan lands as exactly two commits (implementation + merge). Flip `[merge] squash = false` in `.vizier/config.toml` or pass `--no-squash` when you want the target branch to inherit the full draft-branch history instead.

Commit prompts default to the Linux kernel style: `subsystem: imperative summary`
subjects (≤50 chars), wrapped 72-char bodies that explain the "why," plus a
`Signed-off-by` trailer block. Repositories can swap in their own style via
`.vizier/COMMIT_PROMPT.md` or `[prompts.commit]`.

Need to tune prompts/backends per command? Use `[agents.<scope>.prompts.<kind>]` tables inside `.vizier/config.toml` so a single entry defines the template (inline text or `path`) plus backend/model/reasoning overrides for that scope. For example, `[agents.draft.prompts.implementation_plan]` controls plan drafting while `[agents.review.prompts.review]` governs critique prompts; legacy `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`, `.vizier/REVIEW_PROMPT.md`, and `.vizier/MERGE_CONFLICT_PROMPT.md` remain as fallbacks when no profile is set.

Agent-backed commands now resolve to a concrete `AgentRunner` + `AgentDisplayAdapter` pair per scope. Codex implements both pieces today: the runner shells out to `codex exec` while the display adapter turns Codex's JSON events into Vizier's `[codex] <phase> — <message>` history lines. The config resolver attaches those objects once per command (`[agents.ask]`, `[agents.merge]`, etc.), so future CLI agents just plug in their own runner/adapter without touching `draft → approve → review → merge`. When a scope opts into `backend = "wire"`, Vizier swaps in a fallback adapter so wire events still render coherently as `[wire:<scope>]` progress lines even though no agent runner is present.
