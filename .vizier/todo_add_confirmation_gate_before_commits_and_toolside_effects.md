Context:
- Currently, tool-driven writes (add_todo, update_todo, update_snapshot, delete_todo) immediately append to files via FileTracker::write/delete and are auto-committed by Auditor::commit_audit().
- There's no user confirmation step; "force_action" flag only influences provider behavior, not write gating.
- Request: add a confirmation option before the agent commits changes.

Task:
1) Introduce a config flag confirm_before_commit (default: true) in cli/src/config.rs.
   - Expose CLI switches: --yes/-y to bypass confirmations (sets confirm_before_commit=false), and --confirm (explicit true).
2) Gate all write-side effects from prompts/tools.rs behind a Confirm Gate:
   - Add a new function in prompts/src/file_tracking.rs: request_confirmation(kind: &str, target: &str, preview: &str) -> bool
     that:
     - If confirm_before_commit is false: return true immediately.
     - Otherwise, prints a clear, single-line prompt to stderr with kind (ADD/UPDATE/DELETE/SNAPSHOT), target path/name, and a compact preview (first 120 chars, sanitized to one line). Accepts y/N from stdin. Only proceed when user types 'y' or 'yes'.
   - Each tool that mutates state must call this function before writing:
     - add_todo: kind="ADD", target=filename, preview=first line of description
     - delete_todo: kind="DELETE", target=filename, preview="--"
     - update_todo: kind="UPDATE", target=filename, preview=first line of update
     - update_snapshot: kind="SNAPSHOT", target=filename, preview=first line of content
   - On denial, return a structured tool error string: <error>Action canceled by user</error>
3) Gate Auditor::commit_audit() with a final confirmation if there are pending tracked changes and confirm_before_commit is true:
   - Compute and show a short diffstat for .vizier/ and ask: "Commit N changes to .vizier/? [y/N]". Only commit on yes. If no, clear the file tracker pending set without committing.
4) Update cli/src/main.rs help and usage to document -y/--yes and --confirm, and wire to config.

Constraints:
- Keep the flow non-interactive when -y/--yes is set (useful for CI).
- Avoid pulling in heavy crates; use std::io for prompts; fall back to deny on read errors when confirmation required.
- Do not block when running in a non-TTY with confirm=true: detect if stdin is a TTY; if not, deny with an error message suggesting -y.

Acceptance:
- Running a tool that writes without -y prompts for confirmation and cancels on anything but yes.
- With -y, tools proceed and Auditor::commit_audit() auto-commits as before.
- Help text reflects the new flags.
- No confirmations for read-only tools.