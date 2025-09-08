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
