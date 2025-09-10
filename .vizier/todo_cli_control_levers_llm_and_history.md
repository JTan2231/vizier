Context: Users need finer-grained control for both human and non-human CLI interaction. Current config (vizier-core/src/config.rs) only exposes provider + force_action, and there is no explicit history/confirmation/revert machinery surfaced in CLI/TUI.

Tasks:

- Extend Config to encode LLM and control levers (vizier-core/src/config.rs)
  - Add fields with sensible defaults:
    - model: wire::types::API (keep existing provider but add explicit model selection if API supports variants)
    - temperature: f32 (default 0.2)
    - top_p: f32 (default 1.0)
    - max_tokens: u32 (default 4096)
    - system_prompt_overrides: Option<String> (None)
    - history_limit: usize (default 50) — max conversation turns retained
    - confirm_destructive: bool (default true) — requires explicit confirmation before write operations
    - auto_commit: bool (default false) — gate VCS commits behind confirmation unless true
    - enable_snapshots: bool (default true) — take pre/post op snapshots for revert
    - non_interactive_mode: bool (default false) — allows headless automation to proceed with confirm gates via flags
  - Implement Default with these new fields and preserve backward compatibility by mapping old provider/force_action.
  - Update get_system_prompt() to include a <config> block that serializes relevant levers (without secrets) so the model understands runtime constraints.

- Wire CLI flags to config (vizier-cli/src/main.rs)
  - Add flags:
    --provider, --model, --temperature, --top-p, --max-tokens
    --history-limit, --confirm-destructive/--no-confirm-destructive
    --auto-commit, --no-auto-commit
    --enable-snapshots/--disable-snapshots
    --non-interactive
    --system-prompt-override <path|inline>
  - On startup, parse flags, merge into Config, and call set_config(). Ensure numeric validation ranges (temperature 0..=2, top_p 0..=1, max_tokens reasonable bounds) and fall back to defaults with warnings.

- History control and confirmations (vizier-core)
  - Introduce an Operation struct and History ring buffer (size = history_limit) that records:
    - description, affected files, diff or patch, timestamp, reversible flag
  - Provide functions:
    - record_operation(op: Operation)
    - require_confirmation(op: &Operation) -> bool (respects confirm_destructive and non_interactive_mode)
    - revert_last(n: usize) -> Result<(), Error> that uses stored patches (or git) to roll back
  - Expose these via tools module so TUI/CLI can trigger confirm/revert flows.

- TUI affordances (vizier-tui/src/chat.rs + lib.rs)
  - Add UI toggles for confirmation prompts when an operation will write to disk or commit
  - Add a simple History sidebar: list last N operations with ability to revert one
  - Show current LLM settings in a status line with a hotkey to cycle model/temperature presets

- Non-human (headless) operation
  - Define a JSON control schema (vizier-cli README) for non-interactive runs that maps 1:1 to new Config and offers an allowlist of operations (e.g., {"allowWrites":true,"allowCommits":false,"maxEdits":10}). Implement --config-json <path> to load and merge.

Acceptance criteria:
- Running `vizier --temperature 0.7 --history-limit 10 --non-interactive --no-auto-commit` updates the prompt’s <meta><config> block and enforces confirmation gates (skipped only if non-interactive with explicit allow).
- TUI shows history and can revert last operation.
- Non-interactive run fails fast on attempted destructive ops unless allowlist permits.
Add configurable LLM and control levers, history/confirm/revert flows, and non-interactive allowances across CLI/TUI.
Describe user-visible behavior to allow fine-grained control for both interactive and headless usage, exposing settings in a prompt <config> block and enforcing confirmation and history-based revert. Integrate CLI flags and a JSON config for headless runs. (thread: control-levers)

Acceptance Criteria:
- Configurable levers:
  - Users can set provider, model, temperature, top_p, max_tokens, history_limit, confirm_destructive, auto_commit, enable_snapshots, non_interactive_mode, and system_prompt_overrides.
  - Defaults apply when unspecified; existing configs with only provider/force_action remain valid.
  - The system prompt includes a <config> block reflecting effective (non-secret) settings.
- CLI integration:
  - Running: vizier --temperature 0.7 --history-limit 10 --non-interactive --no-auto-commit updates the <config> block and enforces confirmation gates (skipped only when non-interactive and explicitly allowed).
  - Flags supported: --provider, --model, --temperature, --top-p, --max-tokens, --history-limit, --confirm-destructive/--no-confirm-destructive, --auto-commit/--no-auto-commit, --enable-snapshots/--disable-snapshots, --non-interactive, --system-prompt-override <path|inline>, --config-json <path>.
  - Numeric inputs are validated (temperature 0..=2, top_p 0..=1, max_tokens bounded); invalid values fall back to defaults with a warning.
- History and confirmations:
  - The system records each write/commit-capable operation with description, affected files, diff/patch, timestamp, and reversible flag, honoring history_limit.
  - Before destructive actions, a confirmation is requested when confirm_destructive is true; behavior respects non_interactive_mode and an explicit allowlist.
  - Users can revert the last N reversible operations; successful revert restores files with no partial writes.
- TUI affordances:
  - A confirmation prompt appears before disk writes or commits unless explicitly allowed.
  - A History sidebar lists recent operations up to history_limit and allows reverting a selected one.
  - Status line displays current LLM settings and offers a hotkey to cycle preset model/temperature combinations.
- Headless (non-interactive) runs:
  - A JSON control schema maps to Config and includes an allowlist (e.g., allowWrites, allowCommits, maxEdits).
  - --config-json <path> loads and merges settings; destructive ops fail fast unless permitted by the allowlist.

Pointers:
- vizier-core/src/config.rs (Config fields, defaults, get_system_prompt() <config> block)
- vizier-cli/src/main.rs (flag parsing, validation, set_config, --config-json merge)
- vizier-core (history/confirmation/revert surfaces exported for TUI/CLI)
- vizier-tui/src/chat.rs, vizier-tui/src/lib.rs (confirmation UI, history sidebar, status line)

Implementation Notes (safety/correctness):
- Reverts must be atomic and leave no partial writes; use pre/post snapshots or VCS where available. Enforce bounded history storage and avoid secrets in the <config> block. (snapshot: current-config-minimal)Add configurable control levers, history/confirm/revert flows, and headless allowances; surface in <config> and enforce across CLI/TUI.

Describe:
- Extend Config to include: model, temperature, top_p, max_tokens, system_prompt_overrides, history_limit, confirm_destructive, auto_commit, enable_snapshots, non_interactive_mode. Preserve backward compatibility by mapping existing provider/force_action. Update get_system_prompt() to embed a <config> block (no secrets) so the model understands runtime constraints. (thread: CLI/TUI surface area; snapshot: Next concrete moves 1)
- Wire CLI flags to set/validate these levers and call set_config(); support --config-json merge for headless runs with an allowlist. (thread: Headless discipline; snapshot: Next concrete moves 2,5)
- Introduce Operation history with ring buffer (size = history_limit), confirmation workflow respecting confirm_destructive and non_interactive_mode, and revert via stored patches or VCS fallback; expose to TUI/CLI. (thread: Operation history + reversibility; snapshot: Next concrete moves 3)
- TUI affordances: status line showing LLM settings, confirmation prompts before writes/commits, and a History sidebar with revert. (thread: CLI/TUI surface area; snapshot: Next concrete moves 4)

Acceptance Criteria:
- Running `vizier --temperature 0.7 --history-limit 10 --non-interactive --no-auto-commit` updates the prompt’s <meta><config> and enforces confirmation gates; destructive ops are blocked unless explicitly allowed in headless mode.
- CLI supports: --provider, --model, --temperature, --top-p, --max-tokens, --history-limit, --confirm-destructive/--no-confirm-destructive, --auto-commit/--no-auto-commit, --enable-snapshots/--disable-snapshots, --non-interactive, --system-prompt-override <path|inline>, --config-json <path>. Invalid numeric inputs fall back to defaults with warnings (temperature 0..=2, top_p 0..=1, bounded max_tokens).
- History and confirmations: operations record description, affected files, diff/patch, timestamp, reversible flag; ring buffer honors history_limit. Confirmation is required before destructive actions when confirm_destructive is true; behavior respects non_interactive_mode and allowlist. Users can revert the last N reversible operations; revert is atomic with no partial writes.
- TUI: shows current LLM settings in status, prompts for confirmation before writes/commits (unless allowed), and provides a History sidebar listing recent operations with the ability to revert one.
- Headless runs: JSON control schema maps 1:1 to Config and includes allowlist (e.g., allowWrites, allowCommits, maxEdits). --config-json merges settings; destructive ops fail fast unless permitted.

Pointers:
- vizier-core/src/config.rs (Config fields, Default/back-compat, get_system_prompt() <config>)
- vizier-cli/src/main.rs (flag parsing/validation, set_config, --config-json merge)
- vizier-core history.rs + tools.rs (Operation, ring buffer, confirmation/revert APIs)
- vizier-tui/src/chat.rs (status line, confirmation prompts, History sidebar)

Implementation Notes (safety/correctness) (snapshot: Next concrete moves 3):
- Reverts must be atomic; prefer patch apply with VCS fallback. Enforce bounded history and exclude secrets from <config>.