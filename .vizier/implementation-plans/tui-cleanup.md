---
plan: tui-cleanup
branch: draft/tui-cleanup
status: draft
created_at: 2025-11-15T17:19:54Z
spec_source: inline
---

## Operator Spec
all TUI surfaces that are not part of the standard CLI need to be removed. this is primarily ratatui. the functionality provided by these (chat, editor) should not be replaced. removed wholesale without replacement.

## Implementation Plan
## Overview
Vizier no longer needs any alt-screen TUI surfaces. The spec calls for removing the ratatui-based chat and editor experiences entirely instead of replacing them with new UI. This change affects anyone relying on `vizier chat` or the commit-confirmation editor flow; after the cleanup, operators will drive everything through the existing CLI commands (`vizier ask`, `vizier save`, etc.) and commit messages will be generated automatically. Removing these surfaces also lets us drop the unused prompts, config flags, and dependencies they pulled in, which simplifies the product story and resolves the lingering chat-TUI TODO thread.

## Execution Plan
1. **Retire the chat command and module**
   - Delete `vizier-core/src/chat.rs` and remove `pub mod chat` plus any exports from `vizier-core/src/lib.rs`.
   - Strip the `Chat` subcommand (struct, Clap metadata, match arm) from `vizier-cli/src/main.rs` so `vizier chat` is no longer part of the CLI surface.
   - Remove the `CHAT_PROMPT` constant and the `SystemPrompt::Chat` variant from `vizier-core/src/lib.rs`/`src/config.rs`, along with the config parsing helpers that attempted to load `CHAT_PROMPT.md` overrides.
   - Update docs to stop referencing `vizier chat` (Quick Start snippet, Core Commands list, Terminal UI capability bullets, `vizier chat --system-prompt-override` example). Acceptance: `rg "vizier chat"` only returns historical references inside changelog-quality sections (if any) that clearly explain the command’s removal.
   - Refresh `.vizier/.snapshot` so the “Code state” paragraph that currently says “chat TUI lives in vizier-core/src/chat.rs (alt-screen via ratatui)” now documents that the TUI entry point has been removed.

2. **Remove the editor TUI and commit-confirmation plumbing**
   - Delete `vizier-core/src/editor.rs` and drop `pub mod editor` from `vizier-core/src/lib.rs`.
   - Remove the `EDITOR_PROMPT` constant, `SystemPrompt::Editor` enum variant, prompt storage entries, and key-path parsing logic in `vizier-core/src/config.rs`; adjust the config unit tests accordingly.
   - Rip out the `commit_confirmation` flag end-to-end: delete the CLI flag (`--require-confirmation`), the config field, and every call site that performed `if config.commit_confirmation { run_editor(...) }` across `vizier-core::auditor`, `vizier-core::file_tracking`, and `vizier-cli::actions::{run_save, run_approve, refresh_plan_branch}`.
   - Remove the TUI-specific tool glue (`tools::SENDER`, the `edit_content` tool, `get_editor_tools`, and `active_editor_tooling`), since nothing will invoke editor-specific tools once the module is gone.
   - Acceptance: `rg "run_editor"` and `rg "commit_confirmation"` both return zero matches, and the remaining commit flows behave as before but without a confirmation detour.

3. **Prune ratatui-dependent dependencies and build wiring**
   - Update `vizier-core/Cargo.toml` (and `Cargo.lock`) to drop the `ratatui` crate entirely; remove unused imports such as `tokio::sync::mpsc` in `vizier-core/src/tools.rs` that only served the TUI.
   - Double-check that no other module (e.g., `vizier-core/src/display.rs`) still depends on ratatui-specific types; the terminal spinner already uses plain crossterm, so only chat/editor code paths should disappear.
   - Remove any residual references to ratatui or alt-screen behavior from comments and docs so future readers aren’t steered toward non-existent modules. Acceptance: `rg "ratatui"` yields no hits.

4. **Documentation, prompts, and TODO hygiene**
   - Update `README.md` capability sections to describe a CLI-only workflow (Quick Start, Core Commands, Capabilities/Terminal UI paragraphs, configuration examples). Highlight `vizier ask`/`vizier save` as the supported flows and mention that chat mode was removed if needed.
   - Adjust the prompt text in `vizier-core/src/lib.rs` (e.g., the REVISE_TODO example that references “vizier-tui/src/chat.rs” and “CLI flag --confirm/--no-confirm”) so it no longer points to removed files or flags.
   - Close or rewrite `.vizier/todo_chat_tui_auditor_integration_and_commit_gate.md` to document that the chat TUI path has been removed rather than improved, and make sure other TODO threads don’t promise UI affordances that no longer exist.
   - Acceptance: snapshot + README accurately describe the CLI-only posture, and TODOs no longer direct engineers to work on the removed TUI.

## Risks & Unknowns
- **Config compatibility**: Existing configs may still set `commit_confirmation` or include `CHAT_PROMPT`/`EDITOR_PROMPT` overrides. We’ll ignore these keys silently, but we should double-check that serde/loading code doesn’t panic when those entries remain.
- **Docs drift**: Removing a top-level command affects operator mental models; we must ensure every reference (README, prompts, snapshot, TODO threads) is updated so reviewers don’t assume the TUI still exists.
- **Downstream automation**: Scripts or docs that invoked `vizier chat` will now fail; consider mentioning the removal in release notes/outcome text so operators aren’t surprised.

## Testing & Verification
- `cargo fmt` to keep the workspace consistent after deleting large modules.
- `cargo test --workspace` to ensure config-unit tests and CLI/vcs integration tests still pass without the editor/chat code paths.
- `cargo clippy --workspace --all-targets` (or at least `cargo check`) to confirm that removing the modules and dependencies leaves no dead imports or warnings.
- `cargo run -- --help` (or equivalent) to confirm the CLI help text no longer lists the `chat` subcommand or `--require-confirmation` flag.

## Notes
- Drafted the CLI-only cleanup plan described above and saved it to `.vizier/implementation-plans/tui-cleanup.md` on `draft/tui-cleanup` for reviewer sign-off.
