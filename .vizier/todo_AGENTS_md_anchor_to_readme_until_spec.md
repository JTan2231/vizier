# Keep AGENTS.md in sync with README identity/capabilities until spec.json exists

## Goal
Avoid divergence. For now, AGENTS.md should mirror the README’s Who/What and link back to it, with a short note that a machine-readable spec is coming.

## Tasks
- Insert a top section in AGENTS.md that:
  - Quotes or references the README “Who am I / What can I do?”
  - States that `agents/spec.json` will be the canonical machine-readable interface once introduced.
  - Provides current guidance for agents: call CLI with single-shot commands, parse stdout summaries, honor exit codes.

## Acceptance
- AGENTS.md front-matter matches README language closely.
- Clear “coming soon” pointer to `agents/spec.json` and `vizier describe`.
