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

