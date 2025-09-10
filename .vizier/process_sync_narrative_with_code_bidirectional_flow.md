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

Add system prompt anchors and drift guardrails; expose contract levers and CLI affordances.

Describe:
- Extend get_system_prompt() to render <config> (control levers per snapshot) and <narrative_contract> with snapshot_version (hash of .vizier/.snapshot), active_todos (names + short IDs), and contract_mode. Exclude secrets. Preserve legacy meta: fileTree, todos, cwd. (snapshot: Recent decisions; thread: Narrative contract + drift guardrails)
- Implement auditor::check_contract_alignment() comparing current .snapshot digest to session snapshot_version. On divergence during write/commit flows, surface NarrativeDrift with resolution options: open snapshot diff patch and/or TODO update patch. Add vcs pre-commit hook installer honoring drift_block_level and commit override trailer “Narrative-Drift: accepted.” (thread: Narrative contract + drift guardrails)
- Add contract levers to Config: contract_mode (Advisory|Strict), drift_block_level (none|warn|block; default warn), require_linked_todo_on_destructive (default true). Wire CLI subcommands: vizier contract status | accept-drift; vizier todo resolve/link. Reflect current state in the prompt’s <narrative_contract>. (thread: Control levers in Config; CLI/TUI surface area)

Acceptance Criteria:
- get_system_prompt() includes <config> and <narrative_contract> alongside legacy meta, with no secrets; snapshot_version matches the normalized digest of .vizier/.snapshot; active_todos list is present.
- In Strict mode with drift_block_level=block, attempts to commit behavior changes without corresponding snapshot/TODO updates are blocked by the pre-commit hook unless the commit includes those updates, the message has “Narrative-Drift: accepted,” or the user passes --allow-drift.
- Running `vizier contract status` shows a concise alignment report (aligned/warn/blocked), displays current vs session snapshot digests, lists unlinked changes, and offers to apply suggested fixes (open snapshot/TODO patches) interactively.

Pointers:
- vizier-core/src/config.rs, vizier-core/src/display.rs (prompt anchors)
- vizier-core/src/auditor.rs (check_contract_alignment), vizier-core/src/vcs.rs (pre-commit hook)
- vizier-cli/src/main.rs (contract/todo subcommands, --allow-drift)

Implementation Notes (safety/correctness):
- Hash normalized .snapshot (ignore whitespace-only diffs) for stable snapshot_version.
- Auditor should operate on staged content during pre-commit and provide deterministic patches with clear remediation messages.