# Stage Execution

The current CLI executes composed workflow stages through `vizier run` plus scheduler/runtime primitives.

## What Runs Now

- `init`: validates or writes repository bootstrap markers.
- `list`: reads pending plan metadata from git branches and plan docs.
- `jobs`: reads and operates on persisted job records.
- `run`: resolves a flow source, compiles workflow nodes, enqueues scheduler jobs, and optionally follows terminal state.
- `release`: computes release bump/notes and writes release artifacts.

## Operational Notes

- Job log streaming is command-local: use `vizier jobs tail <job> --follow`.
- Help output is pager-aware on TTY and plain in non-TTY contexts.
- Removed workflow-global runtime flags are no longer supported.
