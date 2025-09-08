Refinement (2025-09-08) — Align with current code and snapshot

- System prompt anchors
  • Files: vizier-core/src/config.rs, vizier-core/src/display.rs
  • Action: Extend get_system_prompt() to render both <config> (new control levers per snapshot) and <narrative_contract> { snapshot_version, active_todos, contract_mode }. Ensure no secrets in either block. Keep legacy meta (fileTree, todos list, cwd).

- Auditor drift checks
  • Files: vizier-core/src/auditor.rs, vizier-core/src/vcs.rs
  • Action: Implement check_contract_alignment() that hashes .vizier/.snapshot and compares with session snapshot_version. On divergence during write/commit flows, surface NarrativeDrift with resolution options: (a) open snapshot diff patch; (b) open TODO update patch. Provide a pre-commit hook installer in vcs.rs respecting drift_block_level and an override trailer Narrative-Drift: accepted.

- Config levers and CLI affordances
  • Files: vizier-core/src/config.rs, vizier-cli/src/main.rs
  • Action: Add contract_mode (Advisory|Strict), drift_block_level (none|warn|block; default warn), require_linked_todo_on_destructive (default true). Wire CLI: vizier contract status | accept-drift; todo resolve/link. Validate and reflect state in prompt.

Acceptance additions:
- get_system_prompt() contains <narrative_contract> alongside <config> and legacy meta.
- In Strict+block mode, committing behavior changes without snapshot/TODO updates is prevented unless user includes override trailer or --allow-drift.
- `vizier contract status` prints a concise alignment report with suggested fixes and the option to apply.


---

