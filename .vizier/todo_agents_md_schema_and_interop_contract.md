# AGENTS.md schema and interop contract

Thread: Agent Decision Log + Interop via AGENTS.md (NEW). Cross-links: Architectural/Thematic Invariants; Session logging; Outcome summaries; Agent Basic Command.

Tension
- Product and architectural decisions made during agent operations are implicit and ephemeral, forcing users to rehash context to other agents/tools. We need a durable, machine- and human-readable artifact (AGENTS.md) that captures the agent-facing contract and recent decisions so other agents (e.g., Codex/Claude) can act "from the vizier" with minimal prompting.

Proposed behavior
- Introduce a repository-root AGENTS.md that serves two roles:
  1) Contract: Stable, concise prompts + constraints the Vizier agent expects peers to honor (invariants, output contracts, safety gates, autonomy posture).
  2) Changelog: Append-only, summarized decisions/outcomes from recent agent-driven actions, focused on product/architectural intent and user-visible contracts.
- Provide a minimal, schema-like structure in markdown that other agents can reliably parse or skim.

Acceptance criteria
1) Presence and discovery
   - If AGENTS.md exists at repo root or .vizier/AGENTS.md, CLI surfaces its path in meta/header.
   - `vizier init` offers to scaffold AGENTS.md with sections and examples.

2) Structure (stable headings)
   - Title and Version (AGENTS.md v1)
   - Contract
     - Operating Posture (Default-Action, commit isolation, gates)
     - Output Contracts (stdout/stderr, Outcome line, JSON stream availability)
     - Invariants Pointers (paths to invariants files)
     - Control Levers (config keys like thinking_level, auto_commit)
   - Interop Guide
     - How to ask another agent to "do X from the vizier" (inputs to provide; outputs expected)
     - Minimal prompt template with placeholders
   - Decision Log (most recent N=20)
     - Each entry: date, operation, affected surface, decision summary, rationale, links (PR/TODO/snapshot moment)

3) Lifecycle and updates
   - After each agent-driven operation, append a single Decision Log entry with the above fields.
   - Entries are concise (<=10 lines each) and reference TODO/thread IDs.
   - Outcome summary includes the count of updated Decision Log entries when applicable.

4) Interoperability
   - Provide a machine-readable fence for Decision Log entries (e.g., Markdown list items under a stable heading with a simple key: value block) that other agents can parse without brittle heuristics.
   - Document a minimal handoff prompt in the Interop Guide that instructs external agents how to consume Snapshot + AGENTS.md and where to write results.

5) UX touchpoints
   - CLI: `vizier agents show` opens AGENTS.md; `vizier agents append` can add a structured decision entry interactively or from flags (non-interactive mode for CI).
   - Session logs record whether AGENTS.md was updated and the entry ID.

Pointers (orientation only)
- Surfaces: root AGENTS.md or .vizier/AGENTS.md; vizier-cli (new subcommand group); vizier-core/chat.rs (post-action hook); vizier-core/auditor.rs (links/IDs); session logging.

Notes
- Keep schema stable and small; favor predictable headings over bespoke front matter. Avoid prescribing YAML unless we later add a separate machine-only index.Ship AGENTS.md v1 (Contract + Decision Log) and wire CLI interop.
Describe behavior:
- Add a repository-root AGENTS.md that provides a stable contract for agent interop and an append-only Decision Log of agent-driven outcomes. After each agent-driven operation completes, append a concise Decision entry referencing TODO/thread IDs and Outcome facts. The CLI surfaces AGENTS.md via agents commands; Outcome epilogues note when AGENTS.md was updated. (thread: Agent Decision Log + AGENTS.md interop)

Acceptance Criteria:
- Presence and discovery:
  - AGENTS.md exists at repo root (or .vizier/AGENTS.md) after the first agent-driven operation. The CLI meta/header surfaces its path when present.
  - CLI provides agents commands to show and append entries (e.g., vizier agents show, vizier agents append).
- Structure (stable headings):
  - Title and Version (AGENTS.md v1).
  - Contract: Operating Posture (Default-Action, commit isolation, gates), Output Contracts (stdout/stderr, Outcome line, JSON stream), Invariants Pointers, Control Levers (e.g., thinking_level, auto_commit).
  - Interop Guide: how external agents invoke Vizier (inputs to provide; outputs to expect), and a minimal handoff prompt template with placeholders.
  - Decision Log (most recent first): each entry includes {date, operation, affected surface, decision summary, rationale, links to PR/TODO/snapshot moment, referenced thread/TODO IDs}.
- Lifecycle and updates:
  - After each agent-driven operation, exactly one Decision Log entry is appended. Entries are concise (<=10 lines) and parsable via a simple key: value block under the Decision Log heading.
  - Outcome epilogue and outcome.v1 JSON include whether AGENTS.md was updated and the entry count delta when applicable.
  - Session logs record that AGENTS.md was updated and include the entry ID/reference.
- Interoperability:
  - Decision Log entries are machine-friendly: use a stable fenced or list format with predictable keys so other agents can parse without brittle heuristics.
  - Interop Guide documents how to consume Snapshot + AGENTS.md and where to write results or references back.
- Tests:
  - Scaffolding: upon first agent-driven operation, AGENTS.md is created with required sections.
  - Append: running an agent operation appends exactly one well-formed Decision entry.
  - CLI: vizier agents show opens/displays the file; vizier agents append adds a structured entry non-interactively.
  - Outcome/session: Outcome epilogue/JSON indicate AGENTS.md update; session log includes the entry reference.

Pointers:
- AGENTS.md at repo root (or .vizier/AGENTS.md); vizier-cli agents subcommands; vizier-core/auditor.rs (Outcome facts), vizier-core/chat.rs (post-action hook), session logging (artifact references).