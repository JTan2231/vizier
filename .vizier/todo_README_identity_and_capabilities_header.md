# Add "Who am I / What can I do?" header to README

## Goal
Make the first screenful of README answer: Who is Vizier? What can it do? How do I use it now? How will agents integrate?

## Draft content (proposed drop-in)

### Who am I
Vizier is a repo‑native assistant that plans, edits, and audits changes with you in the loop. It understands your tree (files, git history, tests) and works through a tight plan → change → audit loop so you can move fast without losing control.

### What I can do
- Plan concrete steps from a high‑level goal
- Make scoped edits across files and keep diffs tidy
- Run and observe commands/tests; summarize outcomes
- Gate changes with an auditor/commit review
- Explain decisions and current project state on demand

### How to use me (quickstart)
- Install: `curl -fsSL https://.../install.sh | sh` (replace with real URL or repo path)
- Get help: `vizier help`
- Examples:
  - Ask for a plan: `vizier plan "add a describe command to print capabilities"`
  - Apply changes (review diffs first): `vizier apply`
  - Run tests and summarize: `vizier run "cargo test" --summarize`

### How agents can talk to me
Today: drive via CLI/stdio; prefer structured, single‑command invocations and parse stdout for summaries.
Soon: a machine‑readable spec (agents/spec.json) exposing actions, inputs/outputs, and protocol mode.
Track progress: see AGENTS.md.

## Acceptance
- Header appears at top of README and renders within first screenful on GitHub.
- Language matches snippet (allowing minor tweaks for URL/commands).
- No conflicting description elsewhere in README.
