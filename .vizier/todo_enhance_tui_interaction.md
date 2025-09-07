- **Context:** The TUI (Text-based User Interface) currently lacks depth and clarity in its potential scope and functionality. Stakeholders have pointed out the need to evaluate how far the TUI should evolve — specifically questioning if dashboards represent a pinnacle or if other immersive layers could provide richer interactions.

- **Action:** Establish a clear vision for the TUI's future development. Examine existing components and:
  - Outline a "vision document" that defines the TUI's possible trajectory. Aim to balance simplicity and usability against feature richness without losing cohesiveness.
  - Create an interactive prototype of potential dashboard components, focusing on those that enhance storyline coherence without becoming overly reductive.

- **Goal:** Ensure the TUI contributes meaningfully to the user's narrative, providing necessary information elegantly and intuitively. The aim is to define impact-driven plot points that elevate the user's journey through innovation, not mere embellishment.Concrete plot points to resolve current TUI tensions:

1) Persist edits without killing the TUI
- Replace the exit-based flow in tui/src/lib.rs::App::enter_directory(). Currently, opening a file launches user_editor() then calls std::process::exit(0). Instead:
  - Leave alternate screen + disable_raw_mode before launching editor
  - Run editor on a temp file, then write changes back to the original path
  - Re-enter alternate screen + re-enable_raw_mode, refresh list and preview, and continue the loop

2) Make user_editor write back to the selected file
- Change signature to user_editor(original_path: &Path, file_contents: &str) -> io::Result<()>
- After editor exits, read temp content and std::fs::write(original_path, edited)
- Handle errors and surface them in the preview pane (right column)

3) Remove the carriage-return spinner hack
- In tui/src/lib.rs::display_status, replace print!("\r…") usage with crossterm::terminal::Clear(ClearType::CurrentLine) and write the status line once per tick
- Ensure display_status never leaves stray characters; always Clear(CurrentLine) then render

4) Add in-TUI edit shortcut and safe reload
- In list_tui() event loop, map 'e' to edit the selected file path if it is a file (skip directories)
- After return from editor, call app.read_selected_file_content() and redraw

5) Prevent runaway scrolling and improve readability
- Track the visible height of the preview chunk; cap app.scroll so it never exceeds file_lines.saturating_sub(visible_height)
- Add Home (reset scroll to 0) and End (scroll to bottom) keybindings

6) Tighten file listing for TODO browsing
- In App::refresh_files, when browsing the TODO_DIR, filter to *.md files and ignore hidden files; keep current dir-first sorting

Acceptance criteria:
- Editing a TODO returns to the TUI without terminating the process, with changes saved to disk and reflected in the preview
- Spinner/status updates leave no visual artifacts
- Scrolling cannot exceed bounds; Home/End work
- Pressing 'e' in the list pane edits the selected TODO when it is a file

---

Additional necessary fix uncovered while tying editor lifecycle to TUI:

7) Fix duplicate shell args when launching $EDITOR
- In user_editor(), we currently pass shell.get_interactive_args() and then also append .arg("-c"). For Bash/Zsh this results in two "-c" flags; for Fish, we pass "-C" and still add "-c".
- Solution: Make get_interactive_args() return the complete arg vector needed (including the command flag where appropriate) and do not append another "-c" in user_editor(). Validate for Bash/Zsh/Fish.

Acceptance: Opening the editor works in Bash, Zsh, and Fish without duplicated or invalid flags.

---

Updates bound to current code after reading tui/src/lib.rs:

- App::enter_directory() currently disables raw mode, leaves alt screen, calls user_editor(&self.file_content) and then std::process::exit(0). Replace with:
  • Capture selected path; if file, call user_editor(&path, &self.file_content). On Ok, reload via self.read_selected_file_content(); on Err(e), set file_content to format!("Edit failed: {}", e).
  • Before launching editor: disable_raw_mode + LeaveAlternateScreen; after return: EnterAlternateScreen + enable_raw_mode and redraw.

- user_editor signature and shell args:
  • Change to fn user_editor(original_path: &Path, file_contents: &str) -> io::Result<()>
  • Remove the extra .arg("-c"); rely solely on Shell::get_interactive_args() to include the command flag.
  • After editor exit, read temp file and write back to original_path.

- display_status cleanup:
  • Replace print!("\r…") with crossterm::execute!(stdout(), crossterm::terminal::Clear(ClearType::CurrentLine)); then write spinner + message; flush.

- list_tui improvements:
  • Map 'e' to trigger edit for currently selected file (use App::get_selected_file_path()).
  • Add KeyCode::Home to reset scroll=0; KeyCode::End to scroll to bottom using computed bounds.

- Scroll bounds:
  • Track visible height from frame.area() right pane and file_content line count; cap scroll with saturating_sub.

- File filtering in refresh_files():
  • When path ends_with("todos") or matches known TODO dir, include only *.md and exclude dotfiles.

Acceptance unchanged.


---

Refinement after reading current tui/src/lib.rs (confirmed code anchors):

- App::enter_directory(): explicitly remove std::process::exit(0) after user_editor(); surround editor launch with disable_raw_mode + LeaveAlternateScreen before, and EnterAlternateScreen + enable_raw_mode after. On Ok, call self.read_selected_file_content(); on Err(e), set self.file_content = format!("Edit failed: {}", e).

- user_editor(): adjust signature to fn user_editor(original_path: &Path, file_contents: &str) -> io::Result<()>; remove extra .arg("-c") so we rely solely on Shell::get_interactive_args() which should include the command flag. After editor exits, read temp file back and write to original_path.

- display_status(): replace CR hack with crossterm::terminal::Clear(ClearType::CurrentLine) + redraw spinner/message every tick; ensure no artifacts remain.

- list_tui(): map 'e' to launch edit for current file (skip dirs) via App::get_selected_file_path(); add KeyCode::Home to set scroll=0 and KeyCode::End to scroll to bottom using computed bounds from preview height and content lines.

- App::refresh_files(): when browsing TODO dir, filter *.md and ignore dotfiles; keep dir-first sort.

Acceptance unchanged.

---

Anchor acceptance to precise code changes and expand keybindings:

- Keybindings summary to implement in tui/src/lib.rs::list_tui():
  • e — Edit selected file (if file)
  • r — Reload selected file from disk (discard scroll to 0)
  • Home/End — Jump to start/end of file
  • PageUp/PageDown — Scroll by visible_height minus 1

- Preview height bound: compute from layout chunks each render; enforce scroll <= max(0, file_lines.saturating_sub(preview_height)). When file changes on disk after edit, reset scroll to min(current_scroll, new_max_scroll).

- Editor lifecycle resilience: if $EDITOR not set, fallback to `vi` on Unix and `notepad` on Windows; log a Warn into App.errors and proceed.

- TODO dir detection: derive at runtime from env VIZIER_TODO_DIR or default ".vizier/todos"; use this to filter *.md and ignore dotfiles.

- Spinner cleanup: ensure display_status clears line on each tick using crossterm::execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine)); on completion, render a final message without spinner and leave a trailing newline so subsequent frames start clean.

- Shell arg matrix validation (note): update Shell::get_interactive_args() docstring to specify returned args include the command flag (e.g., ["-lc"] for zsh, ["-lc"] for bash, ["-C"] for fish) and that user_editor must not append another flag.

Acceptance remains as previously specified.

---

Refined, code-anchored tasks aligned with current files:

- App::enter_directory(): remove std::process::exit(0) after user_editor(); wrap editor launch with disable_raw_mode + LeaveAlternateScreen before, EnterAlternateScreen + enable_raw_mode after; on success reload selected file, on error display concise failure in preview.
- user_editor(original_path, contents): change signature to accept original path; create temp file, write contents, launch $EDITOR using Shell::get_interactive_args() without appending extra "-c"; on exit, write edited temp back to original_path; return io::Result.
- display_status(): replace CR hack with crossterm::execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine)) then render spinner+message.
- list_tui(): bind 'e' to edit selected file; add Home/End; compute visible preview height each render and clamp scroll to lines.saturating_sub(height).
- App::refresh_files(): when in TODO dir (.vizier/todos by default or VIZIER_TODO_DIR), include only *.md and exclude dotfiles.
- Keybinding doc: e(edit), r(reload), Home/End, PageUp/PageDown (height-1 step).
- Editor fallback: if EDITOR unset, default to vi (Unix) or notepad (Windows); surface warn in UI.

Acceptance unchanged.

---

Refocus from vision to concrete, code-anchored fixes aligned with current tui/src/lib.rs:

- App::enter_directory(): stop terminating the app after user_editor(); wrap editor launch with disable_raw_mode + LeaveAlternateScreen before, EnterAlternateScreen + enable_raw_mode after. On Ok, refresh selected file and preview; on Err(e), display concise failure text in the preview.

- user_editor(original_path, contents): change signature to accept original path, write contents to a temp file, launch $EDITOR using Shell::get_interactive_args() without appending another "-c"; on editor exit, write edited temp back to original_path; return io::Result.

- display_status(): replace carriage-return spinner with crossterm::execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine)) then render spinner/message per tick; ensure no stray glyphs remain.

- list_tui(): bind 'e' to edit currently selected file (if not a dir); add Home/End; compute visible preview height per render and clamp scroll to lines.saturating_sub(height); add PageUp/PageDown steps of height-1.

- App::refresh_files(): when browsing the TODO dir (env VIZIER_TODO_DIR or .vizier/todos), include only *.md and exclude dotfiles; keep dir-first sort.

- Editor fallback: if $EDITOR unset, use vi on Unix and notepad on Windows; surface a warning line in the status area.

Acceptance unchanged: edit returns to TUI with saved changes and updated preview; spinner clean; scroll bounded; 'e' edits selected TODO; shell args work across Bash/Zsh/Fish.

---

Refinement — Editor fallback and error logging aligned with observability thread:

8) Editor fallback + error log plumbing
- If $EDITOR is unset, default to `vi` on Unix and `notepad` on Windows. Show a one-line warning in the TUI status area on first use during a session.
- On editor launch or write-back failure, append a JSON line to .vizier/logs/errors.jsonl with fields: { ts, source: "tui", action: "user_editor", path, message, stderr? }.
- Ensure failures do not crash the TUI; keep the app in a recoverable state and display a concise error in the preview pane.

Acceptance: Missing $EDITOR degrades gracefully with a warning; any editor-related failure is logged to errors.jsonl and surfaced in the UI without terminating the app.


---

Refinement — lock in spinner cleanup, keybound scroll limits, and editor fallback/error logging to match snapshot Thread A:

- display_status(): Always execute Clear(ClearType::CurrentLine) and MoveToColumn(0) before rendering spinner/message; flush stdout each tick. On completion, render final line with trailing newline.
- Scroll bounds: compute preview_height from layout; clamp scroll to lines.saturating_sub(preview_height). Add PageUp/PageDown to move by preview_height-1. Preserve scroll within new bounds after edits/reloads.
- Editor fallback + error logs: if $EDITOR unset use vi (Unix) or notepad (Windows); show one-time warning in status. On any editor launch or write-back failure, append JSONL to .vizier/logs/errors.jsonl with {ts, source:"tui", action:"user_editor", path, message, stderr?}. Do not crash; display concise error in preview.
- Shell arg duplication fix: rely solely on Shell::get_interactive_args() to include correct command flag (-lc for bash/zsh, -C for fish); remove any extra "-c" in user_editor().
- TODO dir filtering: when browsing TODO dir (VIZIER_TODO_DIR or .vizier/todos), include only *.md and exclude dotfiles; keep dir-first sort.

Acceptance unchanged: edit returns to TUI with saved changes and updated preview; spinner clean; scroll bounded; keybindings work across terminals; missing $EDITOR degrades gracefully and logs errors.

---

