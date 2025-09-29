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
- Keep implementation open. Ensure atomic profile writes and clear error reporting in-panel if write fails.