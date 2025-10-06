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

---
Rendering invariants (2025-10-02)
- Declare repository-level rendering invariants to reduce UI regressions:
  1) Terminal-first minimalism: no alternate screen/fullscreen by default; preserve scrollback.
  2) Environment-aware: gate control sequences on TTY; piping/CI emits plain lines.
  3) Stable outcomes: every action ends with a concise Outcome line sourced from Auditor/VCS facts.
  4) Renderer-neutral: core emits a versioned event stream that all surfaces consume.
- Cross-link: Thread “Terminal-first minimal TUI + renderer-neutral events”; see TODO minimal_invasive_tui_and_renderer_neutral_surface for acceptance criteria.


---

Introduce an “Invariants” channel with repo/module/session scopes and surface it in prompts and outcomes (CLI-first).
Describe behavior:
- Provide first-class Architectural/Thematic Invariants that guide assistant actions. Discover invariants at repo level (.vizier/invariants.md), module level (.vizier/invariants.d/<path>.md, nearest ancestor applies), and session level via a CLI flag. Merge the active set and show paths in CLI meta/header; include invariants status in Outcome. Always include the merged set in the system prompt so plans/edits prefer honoring them. TUI affordances are deferred until a UI surface exists. (thread: Control levers surface; cross: Outcome summaries) (snapshot: Running Snapshot — updated)

Acceptance Criteria:
- Discovery/visibility:
  - If .vizier/invariants.md exists, it is recognized; module-level files under .vizier/invariants.d/ map to subtrees; a --invariants <file> flag adds session-level notes.
  - Active invariants paths are listed in the CLI header/meta for ask/chat and referenced in the Outcome epilogue; absence yields “Invariants: N/A”.
- Prompt integration:
  - For assistant actions that may change repo state, the system prompt contains an “Invariants” section with the merged active set. thinking_level influences depth of checking but invariants are always included.
- Outcome checks:
  - Outcome summaries report invariants status as Upheld (default), Flagged (list of short IDs), or N/A. If Flagged, the assistant includes a 1–2 line note referencing which invariant(s) were challenged.
  - outcome.v1 JSON includes invariants: {status: "upheld"|"flagged"|"na", flagged_ids?: []}.
- Auditor hook:
  - Provide a lightweight interface for tools/steps to emit invariant flags (textual with stable IDs). Flags are reflected in Outcome and recorded for session persistence.
- Persistence:
  - Session logs (session.json) record the active invariants paths, merged IDs, and any flags raised.
- UX scope:
  - TUI chips/panes are deferred until a vizier-tui surface exists; product spec remains as target. CLI remains line-oriented and honors stdout/stderr/verbosity rules.
- Tests:
  - Cover precedence/merging across repo/module/session scopes, prompt inclusion, Outcome status rendering (human and JSON), N/A behavior when absent, and session log contents.

Pointers:
- vizier-core/config.rs (load + precedence), vizier-core/chat.rs (system prompt assembly), vizier-core/auditor.rs (flag interface), vizier-cli/src/main.rs and actions.rs (flags + epilogue), session logging hooks.

Implementation Notes (safety/correctness):
- Precedence: session augments/overrides module, which augments/overrides repo; conflicts resolved by stable IDs on headings (e.g., “[ID]” tags). Absence is no-op (N/A). Limit prompt injection to a bounded subset (top N bullets) with an indicator when truncated. Atomic reads; tolerate large files gracefully.