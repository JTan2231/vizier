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

