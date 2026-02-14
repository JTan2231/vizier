# Stage Execution

The current CLI no longer executes plan-stage implementation commands directly.

## What Runs Now

- `init`: validates or writes repository bootstrap markers.
- `list`: reads pending plan metadata from git branches and plan docs.
- `jobs`: reads and operates on persisted job records.
- `release`: computes release bump/notes and writes release artifacts.

## Operational Notes

- Job log streaming is command-local: use `vizier jobs tail <job> --follow`.
- Help output is pager-aware on TTY and plain in non-TTY contexts.
- Removed workflow-global runtime flags are no longer supported.
