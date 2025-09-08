Refined acceptance and code anchors aligned with current files and keybindings implemented in list_tui():

- App::enter_directory(): stop terminating after user_editor(); wrap with disable_raw_mode + LeaveAlternateScreen before, and EnterAlternateScreen + enable_raw_mode after; on success reload selected file and preview; on error set preview to concise failure text.
- user_editor(original_path, contents): accept original path, write contents to temp, launch $EDITOR using Shell::get_interactive_args() without appending another command flag; on exit, write edited temp back to original_path.
- display_status(): replace carriage-return spinner with Clear(ClearType::CurrentLine) + render spinner/message each tick; ensure trailing newline on completion.
- list_tui():
  • e — Edit selected file (skip dirs)
  • r — Reload selected file (reset scroll to 0)
  • Home/End — Jump to start/end
  • PageUp/PageDown — Scroll by visible_height-1
  • Clamp scroll to lines.saturating_sub(visible_height), recompute height per render.
- App::refresh_files(): when in TODO dir (VIZIER_TODO_DIR or .vizier/todos), include only *.md and exclude dotfiles; keep dir-first sort.
- Editor fallback: default to vi (Unix) or notepad (Windows) if $EDITOR unset; surface a warning line in status area.

Acceptance remains: edit returns to TUI with saved changes and updated preview; spinner clean; scroll bounded; 'e' edits selected TODO; shell args valid for Bash/Zsh/Fish.

---

Refinement: add editor fallback and status warning, plus error log plumbing to .vizier/logs/errors.jsonl consistent with Thread C.

- Editor fallback and warning
  • If $EDITOR is unset, default to vi (Unix) or notepad (Windows). Emit a yellow status line: "warn: $EDITOR not set; using <editor>" via display_status and append to App.errors.

- Errors.jsonl integration
  • On any editor spawn failure or write-back IO error, append a JSON line to .vizier/logs/errors.jsonl with fields: { ts, source: "tui", action: "editor_spawn" | "file_write", path, message, stderr? }.

Acceptance addition:
- Missing $EDITOR path shows a single warning and chooses a sensible default; failures are recorded in errors.jsonl.

---

Add concrete code anchors for fallback/editor errors and spinner cleanup integration with audit logs:

- tui/src/lib.rs::user_editor(original_path: &Path)
  • Detect $EDITOR; if None, set editor = vi (unix) or notepad (windows) and set warn flag.
  • Before spawn: crossterm::execute!(LeaveAlternateScreen); disable_raw_mode(); after child exits, re-enter alt screen and enable_raw_mode.
  • On spawn or wait failure: append JSON line to .vizier/logs/errors.jsonl:
    {"ts": <iso8601>, "source":"tui", "action":"editor_spawn"|"editor_wait", "path": original_path, "message": err.to_string(), "stderr": <captured?>}
    and render a single-line status error.
  • After successful edit: write temp contents back to original_path; on IO error, log {action:"file_write"} to errors.jsonl and render failure in preview.
  • Do not append extra shell control flags beyond Shell::get_interactive_args().

- tui/src/lib.rs::display_status()
  • Replace \r updates with Clear(ClearType::CurrentLine) and explicit draw of spinner/message. Ensure final state prints a newline and clears spinner artifacts.

- Acceptance test hints:
  • Launch with EDITOR=cat to simulate no-op edit and verify write-back path.
  • Force editor spawn failure to see a single errors.jsonl entry and a concise UI error.


---

- Replace immediate exit on file edit
  • File: vizier-tui/src/lib.rs
  • Change: In App::enter_directory(), remove std::process::exit(0). After user_editor(), re-enable raw mode and re-enter alternate screen, then call refresh_files() and read_selected_file_content() to redraw.

- Editor writes back and avoids duplicate shell flags
  • File: vizier-tui/src/lib.rs
  • Change: Modify user_editor() to accept (path: &Path, original_contents: &str). Write to tempfile, launch $EDITOR using Shell::get_interactive_args() only (do not append an extra "-c"), then read the tempfile and write back to the original path. If $EDITOR unset, default to vi/notepad and emit a one-time warning in TUI status.

- Bind 'e' to edit and reload
  • File: vizier-tui/src/lib.rs
  • Change: In list_tui(), on KeyCode::Char('e') when selected is a file, call user_editor() and on success reload file content and redraw.

- Bounded scrolling and jump keys
  • File: vizier-tui/src/lib.rs
  • Change: Track preview height from frame.area(); clamp app.scroll to (0..=max_scroll). Implement PageUp/PageDown as height-1 increments and Home/End set to 0/max.

- Focus TODO browsing
  • File: vizier-tui/src/lib.rs
  • Change: In App::refresh_files(), when path ends with .vizier/todos or equals tools::get_todo_dir(), include only *.md and skip dotfiles.

- Error logging
  • File: vizier-tui/src/lib.rs
  • Change: On editor launch failure or write-back error, append event to .vizier/logs/errors.jsonl with {ts, source:"tui", action:"user_editor", path, message} and render concise status message in TUI.


---

Refinement (2025-09-08): Align with Snapshot Thread A and remove duplication with todo_enhance_tui_interaction.md

- App::enter_directory(): delete std::process::exit(0) after user_editor(). Surround editor launch with disable_raw_mode + LeaveAlternateScreen before, EnterAlternateScreen + enable_raw_mode after. On success reload selection via read_selected_file_content(); on error set preview to concise failure and log.

- user_editor(original_path, contents): change signature to accept original path; write contents to tempfile; launch $EDITOR using Shell::get_interactive_args() only (do not append another "-c"); after exit, write edited temp back to original_path. If $EDITOR unset, fallback to vi (Unix) or notepad (Windows) and render a one-time warning.

- Keybindings in list_tui(): e(edit current file), r(reload & reset scroll), Home/End (jump), PageUp/PageDown (height-1). Clamp scroll to lines.saturating_sub(visible_height).

- App::refresh_files(): when browsing TODO dir (env VIZIER_TODO_DIR or .vizier/todos), include only *.md and exclude dotfiles; keep dir-first sort.

- display_status(): replace CR spinner with Clear(ClearType::CurrentLine) + MoveToColumn(0) before rendering; ensure trailing newline on completion.

- Error logging: on editor spawn/wait/write-back failure append to .vizier/logs/errors.jsonl with {ts, source:"tui", action:"user_editor", path, message, stderr?}.

Acceptance unchanged: edit returns to TUI without exiting, saved changes visible; scroll bounded and keybindings work; shell args correct across Bash/Zsh/Fish; errors logged.

---

Refinement (2025-09-08) — Align with Snapshot Thread A and collapse duplication with todo_enhance_tui_interaction.md:

- Remove exit-after-edit
  • File: vizier-tui/src/lib.rs::App::enter_directory()
  • Action: Delete std::process::exit(0) after user_editor(); after editor returns, EnterAlternateScreen + enable_raw_mode; call refresh_files() and read_selected_file_content(); redraw.

- Editor write-back + shell flag fix
  • File: vizier-tui/src/lib.rs::user_editor(original_path: &Path, contents: &str)
  • Action: Write contents to temp, launch $EDITOR using Shell::get_interactive_args() only (no extra "-c"), on return write temp back to original_path. Fallback to vi/notepad if $EDITOR unset; show one-time warning.

- Keybindings and scroll bounds
  • File: vizier-tui/src/lib.rs::list_tui()
  • Action: Bind e(edit), r(reload/reset scroll), Home/End, PageUp/PageDown(height-1). Clamp scroll to lines.saturating_sub(visible_height) computed per render.

- TODO dir filtering
  • File: vizier-tui/src/lib.rs::App::refresh_files()
  • Action: When browsing TODO dir (VIZIER_TODO_DIR or .vizier/todos), include only *.md and exclude dotfiles; keep dir-first sort.

- Status/spinner hygiene
  • File: vizier-tui/src/lib.rs::display_status()
  • Action: Use MoveToColumn(0) + Clear(ClearType::CurrentLine) before rendering; ensure trailing newline on completion.

- Error logging to match observability thread
  • Files: vizier-tui/src/lib.rs (user_editor), vizier-tui/src/chat.rs (on tool errors)
  • Action: Append JSONL to .vizier/logs/errors.jsonl with {ts, source:"tui", action, path, message, stderr?} on failures; surface concise status.

Acceptance: Edit returns to TUI with saved changes; scrolling bounded with page/home/end; 'e' edits current file; no duplicate -c; errors logged.


---

