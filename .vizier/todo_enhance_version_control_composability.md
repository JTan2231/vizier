### Context

Currently, our interaction with version control (e.g., Git) lacks the composable simplicity and modularity that Unix philosophy champions. This may reflect underlying composability concerns within our architecture.

### Action

- Conduct a review of existing Git/version control processes to identify areas lacking modularity and flexibility.
- Develop a set of Unix-inspired command-line utilities or scripts that streamline complex workflows into simpler, composable commands.
- Implement a prototype that allows for script-based automation of common version control tasks, integrating them with our TUI where applicable.

### Goal

Enable users to engage with version control in a manner that emphasizes simplicity, clarity, and flexibility. The initiative should empower users to construct their own workflows through composable interfaces, thus aligning with the project's storytelling ethos of empowering user narratives with meaningful interactions.Tighten the --save pipeline so it composes correctly with git and our tools:

1) Generate the commit message from the final diff
- In cli/src/main.rs under args.save, we currently compute `let diff = prompts::tools::diff();` before invoking the LLM to update snapshot/TODOs, then use that stale diff to craft the commit message.
- Move the `diff()` call to after the snapshot/TODO updates and staging, so the commit covers all changes produced by the assistant.

2) Factor the save flow into a single function with explicit steps
- Extract current inline logic into fn save_project() -> Result<(), Box<dyn Error>> that performs:
  a) Call llm_request_with_tools(..., "Update the snapshot and existing TODOs as needed", ...)
  b) `git add -u`
  c) Recompute `diff()` now that .vizier and any code edits are staged
  d) Generate commit message from COMMIT_PROMPT + new diff
  e) `git commit -m <message>`
- Return any errors; in main() just call save_project()? to preserve composability.

3) Fix usage text to match actual flags
- print_usage() currently documents `-s/--summarize` but the code defines summarize as `-S/--summarize` and reserves `-s/--save` for the save flow.
- Update the help text to display `-S, --summarize` and `-s, --save` correctly.

4) Defensive behavior for missing todos.json in status tools
- update_todo_status/read_todo_status load from todos.json via load_todos(); if the file is absent, calls will error.
- Implement lazy init: if load_todos() Err(NotFound), create an empty map and write todos.json; otherwise propagate real errors.
- Acceptance: First call to update_todo_status works without a preexisting todos.json.

Acceptance criteria:
- Commit message describes the complete set of changes produced by --save, including snapshot/TODO modifications
- `vizier --help` output matches the actual short flags (-S summarize, -s save)
- The save logic is testable as a single function and returns errors instead of panicking
- First-time projects (no todos.json) can mark statuses without crashing

---

Concrete, code-bound changes:

1) Final staged diff drives commit message
- cli/src/main.rs (args.save branch): move prompts::tools::diff() to after LLM-driven snapshot/TODO updates and after `git add -u`. Use this fresh diff to craft the commit message.

2) Extract save flow
- Introduce fn save_project() -> Result<(), Box<dyn std::error::Error>> encapsulating: (a) LLM update to snapshot/TODOs, (b) git add -u, (c) recompute diff, (d) generate commit message via COMMIT_PROMPT + diff, (e) git commit -m. Return errors; main() simply delegates.

3) Help text parity
- cli/src/main.rs::print_usage(): correct flags to show -S/--summarize and -s/--save. Ensure clap/argh match implementation if used.

4) First-run status store
- prompts/src/tools.rs::load_todos(): if file missing, auto-create .vizier/ and an empty todos.json; return Ok(empty map). Ensure update_todo_status/read_todo_status handle this path.

Acceptance unchanged.


---

Tighten implementation details and acceptance around the --save pipeline with concrete anchors:

- cli/src/main.rs (args.save): compute prompts::tools::diff() only after snapshot/TODO updates and after `git add -u` so the commit message reflects the actual staged diff.

- Extract fn save_project() -> Result<(), Box<dyn std::error::Error>> encapsulating:
  a) LLM-driven snapshot/TODO updates
  b) `git add -u`
  c) Recompute diff
  d) Build commit message from COMMIT_PROMPT + fresh diff
  e) `git commit -m <message>`

- print_usage(): correct flags to -S/--summarize and -s/--save to match actual behavior.

- prompts/src/tools.rs::load_todos(): on NotFound, create .vizier/ and an empty todos.json, return Ok(empty). Ensure update_todo_status/read_todo_status paths accept this and do not crash.

Acceptance remains: final commit message covers all changes; help text matches flags; save_project() returns errors; first-run status updates succeed without existing todos.json.

---

Refinement — Help text and audit trailer linkage:

5) Fix help text parity in cli/src/main.rs::print_usage()
- Ensure -S/--summarize and -s/--save are documented exactly as implemented; clarify -m/-M semantics and exclusivity.

6) Audit trailer linkage for traceability
- In save_project(), when prompts::file_tracking::staged_fingerprint() returns Some(anchor), append an "Audit-Anchor: <anchor>" trailer to the generated commit message.

Acceptance: `vizier --help` shows correct flags and message options; commits produced by --save include an Audit-Anchor trailer when available.


---

Refinement — extract save_project(), commit from staged diff, correct help, and add Audit-Anchor trailer (matches snapshot Thread B):

- cli/src/main.rs::save_project(): New function returning Result<(), Box<dyn Error>> encapsulating:
  1) Call into prompts to update snapshot and TODOs
  2) git add -u
  3) Recompute prompts::tools::diff() from the index
  4) Build commit message from COMMIT_PROMPT + fresh diff; if prompts::file_tracking::staged_fingerprint() -> Some(anchor), append "\n\nAudit-Anchor: <anchor>"
  5) git commit -m <message>
- Args path: replace inline save logic with a call to save_project(); bubble up errors instead of panicking.
- print_usage(): Correct flags to show -S/--summarize and -s/--save; document -m/-M semantics and exclusivity.
- prompts/src/tools.rs::load_todos(): on NotFound, create .vizier/ and write an empty todos.json; return Ok(empty). Ensure update_todo_status/read_todo_status handle this without special-casing.

Acceptance unchanged: final commit reflects all LLM-produced changes; help text correct; save_project() testable; first-run status store works; commit includes Audit-Anchor when available.

---

- Commit from final, staged diff and include .vizier updates
  • File: vizier-cli/src/main.rs
  • Change: Rework save() into save_project(): run LLM/tools to update snapshot/TODOs, then call vcs::add_all_or_update() (including .vizier), recompute diff from index (staged), generate commit message from that staged diff, then commit. Do not exclude .vizier in save path.

- Correct CLI help and mutual exclusivity
  • File: vizier-cli/src/main.rs::print_usage()
  • Change: Ensure -m/--commit-message and -M/--commit-message-editor are marked mutually exclusive in usage text and examples use -s/--save consistently.

- Audit-Anchor trailer
  • File: vizier-cli/src/main.rs
  • Change: When building commit message, if vizier_core::file_tracking::staged_fingerprint() returns Some(anchor), append an "Audit-Anchor: <anchor>" trailer via CommitMessageBuilder.

- First-run store bootstrap
  • File: vizier-core/src/tools.rs (or config/observer where load_todos lives after refactor)
  • Change: On missing .vizier or missing todos store, create directories/files and return Ok(empty) instead of error. Keep behavior unified with Thread C bootstrap to avoid duplication.


---

Refinement (2025-09-08): Align with Snapshot Thread B; commit from staged diff, extract save_project(), correct help, bootstrap store, and add Audit-Anchor.

- Commit message from final staged diff (including .vizier)
  • File: vizier-cli/src/main.rs (args.save path)
  • Change: After LLM updates and git add -u, recompute prompts::tools::diff() from the index and use this to generate the commit message. Ensure .vizier changes are included in staging and diff.

- Extract save_project() orchestration
  • File: vizier-cli/src/main.rs
  • Change: Introduce fn save_project() -> Result<(), Box<dyn std::error::Error>> that performs: (1) run tools to update snapshot/TODOs, (2) git add -u, (3) recompute diff from index, (4) build commit message from COMMIT_PROMPT + diff, (5) append Audit-Anchor if available, (6) git commit -m. Main delegates to this function and propagates errors.

- Audit-Anchor trailer linkage
  • File: vizier-cli/src/main.rs
  • Change: If vizier_core::file_tracking::staged_fingerprint() -> Some(anchor), append "\n\nAudit-Anchor: <anchor>" to the commit message before committing.

- CLI help parity and exclusivity
  • File: vizier-cli/src/main.rs::print_usage()
  • Change: Document -S/--summarize and -s/--save correctly; clarify -m/--commit-message and -M/--commit-message-editor are mutually exclusive, and examples use -s/--save consistently.

- First-run status store bootstrap
  • File: vizier-core/src/tools.rs (or current location of load_todos)
  • Change: load_todos(): if store missing, create .vizier/ and an empty todos.json and return Ok(empty). Callers update_todo_status/read_todo_status should work without special-casing.

Acceptance: --save uses staged diff and includes .vizier; help text correct with -m/-M exclusivity; save_project() testable and returns errors; first-run projects don’t crash; commit includes Audit-Anchor when available.

---

