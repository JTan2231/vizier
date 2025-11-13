Title: TODO garbage-collection (GC) on save and post-action

Thread: Editorial housekeeping → Canonicalize duplicated TODO artifacts; reduce parallel/duplicated tracks.
Snapshot dependency: DAP auto-updates by default; Outcome summaries present after operations; commit isolation + gates in place.

Problem (observed):
- The repo accumulates parallel/duplicated TODO files and stale threads. Users report Vizier is not aggressive enough at deleting unnecessary TODOs during save/commit flows, leaving clutter and confusion about the single source of truth.

Product stance (behavioral):
- Introduce TODO GC that runs during:
  1) vizier save (default)
  2) After assistant-authored operations that materially change TODO/thread state (e.g., canonicalization, merge of duplicates) — reported in Outcome.
- Aggressive-by-default policy with safety rails: honor confirm_destructive and Pending Commit gate.

Scope of GC (what gets deleted):
- Exact/near-duplicate TODOs that reference the same thread or acceptance criteria, where a canonical item exists.
- Superseded TODOs whose scope has been merged into a canonical TODO (tracked via cross-links/IDs in snapshot threads).
- Empty or placeholder TODOs created accidentally (e.g., files with only a title or boilerplate, no acceptance criteria or thread link).
- Orphaned TODOs that no longer map to any active thread in the Snapshot and have been inactive across N consecutive saves (N configurable; default 1 for aggressiveness with an opt-out flag).

What does NOT get deleted:
- TODOs explicitly marked keep or pinned.

User-facing affordances:
- Flags:
  - --gc-todos / --no-gc-todos to force/disable for this run (default: enabled on save).
  - --gc-todos-dry-run to print a deletion plan in Outcome without applying.
- Config:
  - confirm_destructive (existing): if true, GC deletions land in Pending Commit and require confirmation.
  - gc_todos.aggressiveness: {aggressive|conservative} (default aggressive).
  - gc_todos.orphan_saves_threshold: integer >=1 (default 1).
  - gc_todos.protect_patterns: glob list for file names to never delete.

Acceptance criteria:
- When running `vizier save` with default settings:
  - Duplicate/superseded/empty/orphaned TODOs are removed in the staged changeset, respecting gates.
  - Outcome (human + outcome.v1 JSON) lists deleted items by filename with reasons {duplicate_of:X, superseded_by:Y, empty, orphaned} and total counts.
  - In non-TTY/protocol mode, no ANSI; NDJSON events include a gc_todos event with details.
- With --gc-todos-dry-run:
  - No files deleted; Outcome includes the same plan with action:"dry-run" and exit code remains success.
- With confirm_destructive=true:
  - Deletions appear as staged removals; user can accept/reject at the gate; Outcome reflects gate_state: pending.
- Pinned/keep-marked TODOs are never deleted; Outcome lists them under skipped: protected.

Pointer-level anchors:
- CLI surface: vizier-cli/src/main.rs (save path); vizier-cli/src/actions.rs (save orchestration)
- Core: vizier-core::auditor (surface facts to Outcome), vizier-core::vcs (stage deletions), vizier-core::tree or file_tracking (enumerate TODOs), vizier-core::config (new gc_todos levers)

Trade space notes (kept open):
- Duplicate detection may use title similarity + thread ID references; exact algorithm is implementation detail as long as reasons are transparent in Outcome.
- Orphan detection keys off Snapshot Threads section; tolerate lag by using threshold.

Tests:
- Integration tests covering: default save with GC, dry-run, opt-out, confirm_destructive gate, protocol mode JSON event, and protection rules.

Cross-links:
- Default-Action Posture (DAP): GC is part of default housekeeping on save; Outcome must reflect actions.
- Outcome summaries: must include GC facts.
- VCS push flows: unchanged (GC never touches remote auth or push strategies).
- Mode split + stdout/stderr contract: GC respects IO rules in both modes.
