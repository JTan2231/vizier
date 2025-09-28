Thread: Control levers surface (snapshot: “Control levers surface — active”) — extend model/session controls to include thinking level with easy switching and persistence.

Goal
- Make it easy to switch the model’s thinking level (e.g., fast/balanced/deep) across CLI and TUI, and persist that choice in a profile so it survives sessions when desired.

Behavior/UX (Product level)
- New config field: thinking_level with allowed values: fast, balanced, deep. Default: balanced.
- CLI:
  • `--thinking-level <level>` selects the level for the current invocation.
  • `--save-profile` (or `--profile <name> --save`) persists current effective config, including thinking_level, to a profile file.
  • Precedence: CLI flags > active session overrides > named profile > default.
- TUI:
  • Header/status shows current thinking level and its source (e.g., [deep • CLI], [balanced • profile]).
  • Quick switcher (e.g., `t` key or menu) cycles levels or opens a small selector; option to “Make default for this profile”.
  • Switching updates the live session immediately; persistence only happens on explicit save/confirm.
- Prompt/meta:
  • The effective thinking_level appears in the prompt <config> block and in chat header meta.
- Persistence model:
  • Profiles stored in a project-local config file (e.g., .vizier/config or profiles/NAME), human-editable.
  • Safe by default: no overwrites without confirm_destructive or explicit save flag.

Acceptance criteria
1) `vizier --thinking-level deep` runs with deep thinking; prompt <config> shows thinking_level: deep; chat header shows [deep • CLI].
2) In TUI, switching level updates the session immediately and is reflected in header; closing and reopening returns to previous profile unless user saved; if saved, the saved level becomes the default for that profile.
3) `vizier --profile team --thinking-level fast --save-profile` writes/updates a profile so subsequent runs of `vizier --profile team` default to fast.
4) Precedence holds: CLI overrides profile; session switch overrides until exit or save.
5) Non-interactive mode respects the set level and includes it in prompt meta; no interactive prompts for saving unless flags present.

Pointers
- vizier-core/src/config.rs (add thinking_level, load/merge precedence, profile IO)
- vizier-cli/src/main.rs (flags, save behavior)
- vizier-core/src/display.rs (header/meta rendering)
- vizier-core/src/chat.rs (TUI switcher affordance)

Implementation Notes (allowed: safety/correctness)
- Persist writes must be atomic (write-temp + rename) to avoid corrupting profiles.
- Validate allowed values strictly; unknown values cause a clear CLI error and list allowed options.
- Include the source-of-truth label (CLI/session/profile/default) with the effective value to improve operator trust.