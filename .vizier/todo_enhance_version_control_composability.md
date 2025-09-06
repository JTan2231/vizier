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

