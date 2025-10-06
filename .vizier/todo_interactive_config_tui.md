Thread: Control levers surface → Config TUI

Why
- Users find editing config files directly unfriendly; want in-app, discoverable controls with clear precedence and safe persistence.

Behavior
- From TUI, open a Config panel (e.g., key: `c` or via a help menu). Panel shows:
  - Effective config values for essential levers (history_limit, confirm_destructive, auto_commit, non_interactive, model params, system_prompt_override, thinking_level, profile name).
  - For each value, indicate source: CLI, session override, profile, default. CLI-sourced values are read-only in the panel.
  - Inline validation and hints (e.g., ranges, enums like thinking_level: fast/balanced/deep).
  - Live preview of changes that affect visible surfaces (e.g., system prompt path and chat header indicators).
- Edit flow:
  - Navigate fields, change values, and see validation in-place.
  - Choose Apply scope: (1) Session only; (2) Persist to profile (select profile or create new). Respect precedence rules (cannot override CLI within session; persisting updates profile used next launch).
  - Confirm dialog before persisting, with a diff-like summary of changes.
- Integration:
  - Chat header/status bar updates immediately to reflect new effective values and their sources.
  - If system prompt path or thinking_level changes, new sessions reflect this; current session shows effective change where possible.

Acceptance Criteria
1) Config panel opens from the TUI and lists effective values with source badges.
2) Fields validate on edit; invalid entries cannot be applied.
3) Applying to Session updates behavior immediately (e.g., header shows new thinking_level and prompt path).
4) Persisting saves to the selected profile; restart or reload uses those values. CLI-specified values remain read-only and unchanged.
5) Attempting to modify a CLI-sourced field in the panel is blocked with an explanation of precedence.
6) Changes present a confirmation summary before writing to disk; cancel leaves files untouched.

Pointers
- Surfaces: vizier-core/src/display.rs (status/header), vizier-core/src/config.rs (schema, precedence, profile IO), vizier-core/src/chat.rs (keybindings/help, panel routing), vizier-cli (flags for profile selection).

Notes
- Keep implementation open. Ensure atomic profile writes and clear error reporting in-panel if write fails.Expose Config inspector/editor with source badges (CLI-first); TUI panel deferred.
Describe behavior:
- Provide a CLI-first way to view and adjust effective config with clear precedence (CLI > session > profile > default). Show current values with source labels; allow edits scoped to Session or persisted to a Profile with validation and confirmation. Changes reflect immediately in chat/meta headers; TUI panel is deferred until a UI surface exists. (thread: Control levers surface; snapshot: Running Snapshot — updated)

Acceptance Criteria:
- Inspect: A CLI command shows effective config values for key levers (history_limit, confirm_destructive, auto_commit, non_interactive, model params, system_prompt_override, thinking_level, profile) with per-key source {cli|session|profile|default}. Output is human-readable by default; `--json` returns a stable machine shape.
- Edit (session scope): A CLI command updates allowed keys for the current session; changes take effect immediately (meta/header reflects new thinking_level and prompt path). CLI-sourced keys remain read-only and are not changed.
- Edit (persisted profile): A CLI command persists validated changes to the selected profile (or creates a new one if specified). On next launch (or reload), those values are effective per precedence. CLI-sourced keys are unchanged.
- Validation + confirmation: Invalid inputs are rejected with helpful messages; persisting changes requires a confirmation step that previews a diff-like summary of keys affected and their scopes.
- Precedence guardrails: Attempting to modify a CLI-sourced field is blocked with an explanation of precedence; session changes cannot override CLI-provided values.
- Output/IO contract: Human output is line-oriented and respects -q/-v/-vv; no ANSI in non-TTY. `--json`/protocol mode emit only structured JSON on stdout; stderr carries diagnostics gated by verbosity.
- Outcome: After apply/persist, an Outcome line summarizes changed keys and scope (e.g., “Config updated: 3 keys (session)”); hidden with --quiet; included in outcome.v1 JSON when requested.
- Tests: Cover inspect vs set (session/profile), validation failures, precedence blocking of CLI-sourced keys, atomic persistence, and output contracts across TTY vs non-TTY and human vs JSON modes.

Pointers:
- vizier-core/src/config.rs (schema, precedence, profile IO), vizier-cli (new config subcommands; flags), vizier-core/src/display.rs (meta/header refresh), vizier-core/src/chat.rs (expose effective config to headers).

Implementation Notes (safety/correctness):
- Persist with atomic writes (temp file + fsync + rename). Record provenance per key. Never modify CLI-sourced values.