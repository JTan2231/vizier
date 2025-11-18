Please observe `.vizier/.snapshot`, the README.md, and the various prompts in lib.rs before planning so you can get a strong feel for what this project is about.
Please also take a careful look around the implementation to understand the architectural and styling patterns before implementating

Need the draft → approve → review → merge choreography? Read `docs/workflows/draft-approve-merge.md` before editing plan branches so you understand how Vizier manages worktrees, commits, review artifacts, and merge sentinels.

Need to hold Codex changes for manual review? Pass the global `--no-commit` flag (or set `[workflow] no_commit_default = true` in `.vizier/config.toml`). Draft/approve/review will keep their temporary worktrees dirty so you can inspect and commit manually. `vizier merge` still requires an actual merge commit, so finalize the draft branch before running it.
