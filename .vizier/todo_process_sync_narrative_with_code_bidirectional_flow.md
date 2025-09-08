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
- Running `vizier contract status` shows a concise report with suggested fixes and the ability to apply them interactively.