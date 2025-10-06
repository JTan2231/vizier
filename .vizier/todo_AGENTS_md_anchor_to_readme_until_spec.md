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
Keep AGENTS.md identity in sync with README until machine spec exists.
Ensure AGENTS.md opens with a brief “Identity and Capabilities” block that mirrors the README’s “Who I am / What I can do,” links back to those sections, and guides agents on current CLI-based interop until a machine-readable spec lands. Include a clear “coming soon” note for agents/spec.json and protocol/JSON streams without claiming availability. (thread: Agent Decision Log + AGENTS.md interop)

Acceptance Criteria:
- AGENTS.md begins with an “Identity and Capabilities” section that:
  - References and links to the README’s identity/capability sections and matches their substance (allow minimal copy edits).
  - States that agents/spec.json will be the canonical machine-readable spec once introduced; until then, AGENTS.md mirrors README and will be kept aligned.
- A “How agents integrate today” subsection advises:
  - Use single-shot CLI invocations (non-interactive), prefer stable stdout summaries, and honor exit codes.
  - Avoid assumptions about ANSI/interactive prompts; treat stderr as diagnostics only.
  - Note that a stable outcome.v1 JSON and event stream are planned; do not require flags that do not yet exist.
- Consistency guardrails:
  - No references to unshipped commands/flags (e.g., no “vizier describe”, no “--mode protocol” until implemented).
  - Language does not contradict README; if README identity text is updated, AGENTS.md front-matter is updated in the same change.
- Links resolve where present (README anchors if available); “coming soon” notes are clearly marked as future-facing.

Pointers:
- AGENTS.md (repo root), README.md (repo root).