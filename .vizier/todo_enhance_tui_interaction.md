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

