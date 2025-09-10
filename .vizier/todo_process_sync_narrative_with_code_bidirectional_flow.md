Context: The project’s code and narrative are drifting because the process does not continuously synchronize them. We need mechanisms so that narrative (snapshot/specs) and code (commits/PRs, config) co-evolve without either being the single source of truth. The goal is a bidirectional feedback loop where each change in one produces concrete, reviewable updates in the other.

Tasks:

- Add "Narrative Contract" header injection in prompts (vizier-core/src/config.rs + display.rs)
  - Emit a <narrative_contract> block alongside <config> in get_system_prompt() that includes:
    - snapshot_version (hash of .snapshot content)
    - active_todos (names + short IDs)
    - contract_mode: {strict|advisory} from Config
  - LLM must acknowledge this contract and propose diffs to either code or snapshot when divergence is detected.

- Create Snapshot Guardrail in auditor.rs
  - Implement check_contract_alignment() that computes a digest of .snapshot and compares with snapshot_version in the session state.
  - When a write operation occurs and digest changed without a paired snapshot/todo update, emit a NarrativeDrift finding with actionable resolution options.
  - Provide auto-fix hooks: produce patch for .snapshot and/or open a TODO update with concrete changes.

- Git hook integration (vizier-core/src/vcs.rs)
  - Add pre-commit hook installer utility that runs auditor::check_contract_alignment().
  - If drift is detected:
    - Block commit unless one of: (a) commit includes snapshot update; (b) commit message has trailer `Narrative-Drift: accepted`.
  - Document trailers and override flag `--allow-drift` in vizier-cli.

- Bidirectional TODO synchronization (vizier-core/src/tools.rs + vizier-cli)
  - Introduce TodoChange enum {Created, Updated, Resolved} with metadata (linked files, rationale, snapshot_refs).
  - On TODO edits via CLI/TUI, write a trailer line `Linked-Commit: <sha>` and update .snapshot with a short summary under a “Recent Changes” section.
  - On code edits (diff observer), suggest TODO updates by extracting tensions (failed invariants, unimplemented branches) and open a prefilled TODO patch. User can accept or modify.

- Config levers to tune contract strictness (vizier-core/src/config.rs)
  - Add fields with defaults:
    - contract_mode: Advisory (default) | Strict
    - drift_block_level: none | warn | block (default warn)
    - require_linked_todo_on_destructive: bool (default true)
  - Render into <narrative_contract> and enforce in auditor + CLI.

- CLI commands (vizier-cli/src/main.rs)
  - `vizier contract status` — show alignment between current working tree and snapshot/todos.
  - `vizier contract accept-drift` — append Narrative-Drift trailer to next commit.
  - `vizier todo resolve <name>` — marks TODO as Resolved, appends trailer, updates snapshot.
  - `vizier todo link <name> --commit <sha>` — backfills linkage.

- TUI affordances (vizier-tui/src/chat.rs)
  - Status bar indicator for contract mode and drift state (green=aligned, yellow=warn, red=blocked).
  - Inline prompts that, upon model proposing code changes, ask: “Update snapshot/TODOs?” with one-key accept.

Acceptance criteria:
- Commits that change core behavior without updating .snapshot or TODOs cause a NarrativeDrift warning; in Strict mode with block level block, the commit is prevented unless overridden.
- The system prompt includes <narrative_contract> with current snapshot hash and active TODO list, and the LLM proposes appropriate snapshot/TODO diffs when plans diverge.
- Running `vizier contract status` shows a concise report with suggested fixes and the ability to apply them interactively.Add narrative contract surfaces and guardrails across prompt, auditor, CLI/TUI, and VCS to keep code and snapshot in lockstep.
Describe:
- Inject a <narrative_contract> block in get_system_prompt() alongside <config>, containing snapshot_version (digest of .snapshot), active_todos (names + short IDs), contract_mode {advisory|strict}. The LLM acknowledges this contract and, when its plan deviates, proposes diffs to code or snapshot/TODOs for review (snapshot: Project trajectory snapshot).
- Add auditor guardrail that detects drift between working tree behavior/edits and the recorded snapshot_version, and emits a NarrativeDrift finding with actionable resolution options, including auto-fix patches for .snapshot and/or TODO updates. Respect contract settings.
- Provide a pre-commit hook installer that runs the auditor and blocks commits per policy unless a paired snapshot/TODO update is present or an override is explicitly provided.
- Add CLI/TUI affordances to surface contract status, apply suggested fixes, and link commits to TODO changes to maintain a bidirectional audit trail.

Acceptance Criteria:
- System prompt includes <narrative_contract> with: current snapshot hash, list of active TODOs (name + short ID), and contract_mode from Config. In interactive sessions where the model proposes changes that would diverge, it outputs suggested snapshot/TODO diffs for user review before proceeding.
- Commits that change observable behavior without corresponding .snapshot/TODO updates cause a NarrativeDrift warning. In Strict mode with drift_block_level=block, the pre-commit hook prevents the commit unless one of: the commit includes snapshot/TODO updates; commit message includes trailer “Narrative-Drift: accepted”; CLI override flag is set.
- Running “vizier contract status” shows: current alignment state (aligned/warn/blocked), the computed .snapshot digest vs session snapshot_version, list of unlinked behavior changes or TODO edits, and offers to apply auto-fix patches interactively.
- TODO edits via CLI/TUI record a trailer “Linked-Commit: <sha>” and append a concise entry to .snapshot under “Recent Changes.” Code edits observed by the tool suggest prefilled TODO updates capturing tensions (e.g., failed invariants, unimplemented branches) for user accept/modify.
- TUI status bar shows contract mode and drift state (green aligned, yellow warn, red blocked). When the model proposes code changes inline, the UI prompts: “Update snapshot/TODOs?” with one-key accept that stages corresponding updates.

Pointers:
- Prompt/config: vizier-core/src/config.rs (get_system_prompt), vizier-core/src/display.rs.
- Auditor: vizier-core/src/auditor.rs (check_contract_alignment).
- VCS integration: vizier-core/src/vcs.rs (pre-commit hook installer).
- CLI: vizier-cli/src/main.rs (contract subcommands; override flag --allow-drift).
- TODO tooling: vizier-core/src/tools.rs (TodoChange enum), CLI/TUI handlers.
- TUI: vizier-tui/src/chat.rs (status bar, inline prompts).

Implementation Notes (safety/correctness; thread: Narrative contract + drift guardrails):
- The snapshot_version must be a stable digest of .snapshot contents; changing whitespace should not cause false positives (normalize before hashing).
- Auditor must run on staged content for pre-commit; ensure deterministic diffing and clear remediation messages.
- Blocking behavior honors config: contract_mode + drift_block_level and require_linked_todo_on_destructive.