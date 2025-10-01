# Architectural/Thematic Invariants: Prompting + Product Surfaces

Thread: Control levers surface + Config-driven prompt customization + Outcome summaries across interfaces
Depends on: Snapshot (Running Snapshot — updated), Code State facts (Auditor, commit gates, config levers), and pending “thinking_level” config.

Tension
- Vizier underweights architectural/thematic invariants of a target project during assistance. The model tends to optimize for immediate diffs vs. preserving cross-cutting constraints (e.g., layering rules, error-handling policy, i18n, logging discipline, security boundaries). This feels like a prompting/config affordance gap and a missing place in the UX to declare and keep these invariants top-of-mind.

Proposed behavior
- Introduce a first-class “Invariants” channel that the assistant must respect and surface at key moments.
- Provide configuration and UX affordances to define and reference invariants at three scopes: repository-global, area/module, and session-specific.
- Ensure assistants and tools receive the active invariant set on each action, and outcome summaries explicitly report whether invariants are upheld or which were challenged.

Acceptance criteria
1) Config surfaces
   - Repo-level: .vizier/invariants.md recognized and loaded if present.
   - Module-level: .vizier/invariants.d/<path>.md files map to subtrees; nearest ancestor applies.
   - Session-level: `--invariants` flag points to a file; TUI allows pasting/editing a session-local invariants note.
   - Active invariants path(s) are visible in the Chat/TUI header/meta and in CLI epilogue.

2) Prompt integration
   - When the assistant proposes changes, the system prompt includes an “Invariants” section with the merged active set and a short instruction to preserve them and prefer reconciling changes to honor them.
   - The “thinking_level” lever influences how thoroughly invariants are checked but the invariants themselves are always included.

3) Outcome checks
   - Post-action outcome summary includes an “Invariants” line: Upheld (default), Flagged (list shortcodes), or N/A if none defined. If Flagged, the assistant must provide a 1–2 line note referencing the invariant ID and the observed tension.
   - Auditor interface: expose a lightweight hook so tools can emit invariant flags (textual; no strict enforcement required initially).

4) UX affordances
   - TUI: In chat view, a compact “Invariants” chip shows count and status; pressing a key opens a read-only pane with the active set.
   - CLI: `vizier ask` prints the active invariants header and includes the status line in the epilogue.

5) Persistence and discovery
   - If .vizier/invariants.md is missing, `vizier init` offers to scaffold one with examples (logging, errors, boundaries, tests-first).
   - Session logs (session.json) record which invariants were active and any flags raised.

Pointers
- Surfaces: vizier-core/config.rs (load + precedence), vizier-core/chat.rs (system prompt assembly), vizier-core/auditor.rs (optional flag emission), vizier-cli/src/main.rs (flags + epilogue), vizier-core/display.rs + TUI chat header (chips/pane).

Implementation Notes (allowed: safety/correctness)
- Precedence: session-level overrides/augments module-level, which overrides/augments repo-level; on conflicts, most specific wins. Merging by stable IDs (markdown headings with [ID] tags) to avoid duplication.
- Must be no-op safe: absence of files yields N/A, not errors. Atomic reads; tolerate large files by truncating prompt injection to top N bullets with a link cue in UI.
