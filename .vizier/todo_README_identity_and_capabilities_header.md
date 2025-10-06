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
Add a concise “Who am I / What can I do?” header to README aligned with current capabilities.
Description:
- Introduce a top-of-file header that, within the first screenful, answers: Who is Vizier, What it can do today (CLI-first), How to use it now, and How agents integrate (today vs coming soon). Content must reflect the current Snapshot: no TUI promises, no unshipped commands/flags, and no speculative install script. Include examples using existing flows (e.g., ask, save); list agent workflow as “coming soon” unless shipped. (snapshot: Running Snapshot — updated; thread: Default-Action Posture (DAP))

Acceptance Criteria:
- Placement: Header block appears at the very top of README.md and renders within the first screenful on GitHub (≈ first 25–30 lines).
- Sections present:
  - “Who I am”: Repo-native assistant that plans/edits/audits with human-in-the-loop and commit isolation.
  - “What I can do (today)”: High-level capabilities consistent with Snapshot (plan/edit within repo context, gate changes, explain decisions); no TUI claims.
  - “How to use me (quickstart)”: Uses existing commands only (e.g., vizier help; vizier ask "..."; vizier save). No “plan/apply/run” examples and no install curl script; link to the project’s install/Getting Started doc if available.
  - “How agents can talk to me”: Today: drive via CLI/stdio; prefer structured outputs where available. Coming soon: protocol/JSON stream and AGENTS.md contract (without naming unavailable flags); link to AGENTS.md if present, otherwise state “coming soon”.
- Consistency guardrails:
  - No references to non-existent commands/flags (e.g., no “vizier plan/apply/run”, no “--mode protocol” until implemented).
  - No promises of a TUI or alt-screen UX.
  - Any “coming soon” items align with active threads (Agent Basic Command, Outcome summaries, protocol/event stream) without prescribing dates.
- No conflicting descriptions remain elsewhere in README; obsolete claims are updated or removed.
- Links/anchors resolve where applicable (e.g., AGENTS.md if present; otherwise the text clearly indicates it will appear).
- Copy is concise and neutral; fits within the first screenful without excessive prose.

Pointers:
- README.md (repo root); link to docs/ or INSTALL/Getting Started if present.
- Cross-links in text: AGENTS.md (when available).