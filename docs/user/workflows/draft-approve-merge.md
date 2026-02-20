# Workflow Guide

This page is the landing guide for Vizier's current CLI surface.

## Available Commands

- `vizier init` / `vizier init --check`: bootstrap and validate repository setup.
- `vizier list`: inspect pending `draft/*` branches relative to target.
- `vizier clean <job-id>`: remove Vizier-managed runtime residue for one job (or whole workflow run scope).
- `vizier run <flow>`: resolve, compile, enqueue, and optionally follow repo-local workflow templates.
- `vizier jobs ...`: inspect and operate on job records (list, schedule, show, status, tail, attach, approve/reject, retry, cancel, gc).
- `vizier release`: prepare release artifacts from commit history.
- `vizier completions <shell>`: install shell completions.

`vizier cd` remains exposed but intentionally returns a deprecation error.

## Related Pages

- `docs/user/workflows/stage-execution.md`
- `docs/user/workflows/gates-and-conflicts.md`
- `docs/user/workflows/alias-run-flow.md`

## Installed References

- `man vizier`
- `man vizier-jobs`
- `man 5 vizier-config`
- `man 7 vizier-workflow`
- `man 7 vizier-workflow-template`
