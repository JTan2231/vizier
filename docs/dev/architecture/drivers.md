# Driver responsibilities

Drivers host all side effects and environment-specific behavior. They wire the pure kernel to concrete I/O, process execution, and UX decisions.

## Responsibilities
- Resolve configuration precedence (global + repo + CLI overrides) and load prompt files.
- Load snapshot/narrative context from the filesystem.
- Execute agents and stream progress/output to the display layer.
- Perform Git/VCS operations (status, diffs, commits, worktrees, merge/conflict handling) via in-process `libgit2` helpers under `vizier-core/src/vcs/` rather than shelling out to the Git CLI.
- Render TTY output, manage pagers, and apply verbosity rules.
- Persist session logs, job records, and other artifacts.

## Current mapping
- `vizier-kernel/`: pure domain logic (config schema/defaults/merge, prompt assembly, scheduler semantics, audit types, port traits).
- `vizier-core/`: driver host implementing config loading, agent execution, VCS/FS helpers, display, session logging, scheduler/job/workflow runtime orchestration, and plan persistence helpers.
- `vizier-cli/`: frontend wiring (CLI args/dispatch, jobs UX rendering, and `run` operator summaries).

## Side-effectful modules (driver-owned)
- `vizier-core/src/config/load.rs` + `vizier-core/src/config/driver.rs` (config resolution, prompt file discovery, agent runtime wiring)
- `vizier-core/src/agent.rs` (agent execution and progress streaming)
- `vizier-core/src/jobs/mod.rs` + `vizier-core/src/plan.rs` (scheduler/job persistence + workflow runtime bridge and plan record side effects)
- `vizier-core/src/vcs/` (git operations and commit/worktree helpers)
- `vizier-core/src/display.rs`, `vizier-core/src/observer.rs` (TTY rendering, stdout/stderr capture)
- `vizier-core/src/tree.rs`, `vizier-core/src/walker.rs` (filesystem traversal/search)

Drivers should keep kernel purity intact by passing preloaded data into kernel APIs and implementing port traits where needed.
