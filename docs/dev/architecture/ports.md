# Kernel ports

The kernel stays pure by delegating all side effects to a small set of port traits. These live in `vizier-kernel/src/ports.rs` and define the minimum contract drivers must satisfy.

## FsPort
Filesystem access for text and directory operations.
- `read_to_string`, `write_string`
- `list_dir`, `exists`, `canonicalize`

## VcsPort
Version-control access for diffing and status.
- `diff` (base/head + optional path filter)
- `log` (recent commit summaries)
- `status` (branch name + clean/ahead/behind)
- `head` (current commit id)
- `origin` (owner/repo)
- `create_worktree`, `commit`

## ClockPort
Time sources used for timestamps and durations.
- `now_rfc3339`
- `monotonic_ms`

## AgentPort
Agent execution surface for drivers.
- `run(AgentRequest, EventSink) -> AgentResponse`
- The kernel stays agnostic to how progress is streamed; drivers are responsible for wiring events to `EventSink`.

## SchedulerStore
Persistence + lookup for scheduler decisions.
- `load_facts`
- `persist_decision`
- `record_artifact`

## EventSink
Best-effort telemetry stream for drivers.
- `info`, `warn`, `progress`

## Guarantees
- Ports are intentionally thin: they should not embed business logic.
- Errors are returned, not swallowed; callers decide how to surface them.
- The kernel accepts preloaded data rather than reaching into ports directly, keeping deterministic logic pure.

## Implementations
- Drivers in `vizier-core` provide the concrete implementations.
- `vizier-cli` uses `vizier-core` to fulfill port requirements for the CLI frontend.
