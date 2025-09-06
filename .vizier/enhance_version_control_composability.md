Link: This should leverage the new CommitTransaction primitive to make composition predictable. All higher-level workflows (batch file updates, AI-applied refactors) must express intentions as planned IndexEdit/TreeWrite operations and defer execution. Avoid duplicating transaction management here; consume it.

---

Resolve stale-diff commits and make --save composable with git, grounded in current CLI code.

Concrete tasks:

1) Commit message from final staged diff
- File: cli/src/main.rs (save path)
- Move prompts::tools::diff() and commit-message generation to occur AFTER:
  • LLM updates snapshot/TODOs
  • git add -u (and explicit adds for .vizier/* if needed)
- Recompute diff from the index against target ref/range and feed that to the commit-writer.

2) Factor save flow for reuse and testing
- File: cli/src/main.rs
- Extract into fn save_project(args: &Args) -> anyhow::Result<()> that performs:
  a) Run tools to update snapshot/TODOs
  b) git add -u (respect .vizier staging rules)
  c) Recompute diff for code paths (exclude .vizier) against args.save ref/range
  d) Generate conventional commit message (with optional -m/-M author note)
  e) git commit for code changes (skip if diff empty)
- Return Result instead of panicking; log errors through auditor.

3) Help text correctness
- File: cli/src/main.rs::print_usage()
- Ensure flags and descriptions match README and behavior:
  • -s, --save <REF|RANGE>
  • -S, --save-latest (alias for -s HEAD)
  • -m/-M semantics and exclusivity

4) First-run robustness for status store
- File: prompts/src/tools.rs
- In load_todos(): if .vizier/ or todos.json missing, create directory, write empty map to todos.json, return Ok(empty). Callers (update_todo_status/read_todo_status) must not error on missing store.

Acceptance:
- --save produces a commit message reflecting all staged changes after tool updates.
- `vizier --help` displays correct -s/-S semantics and -m/-M details.
- save_project() is unit-testable and returns errors instead of exiting.
- Fresh repos (no .vizier/) do not crash when updating/reading todo status.

Notes:
- Keep conversation commit and .vizier commit separation intact; ensure code commit excludes .vizier paths.
- Where possible, prefer libgit2 for index/diff fidelity; otherwise, shell out consistently and capture stderr for auditor logs.

---

Tighten save flow acceptance and add Git trailer linkage hook to align with audit threads.

- After staging, recompute diff and inject an Audit-Anchor trailer into the commit message when available.
  • File: cli/src/main.rs::save_project()
  • Compute anchor via prompts::file_tracking::staged_fingerprint() if present; append to commit message as a trailer line: "Audit-Anchor: <anchor>". This binds commits to audit sessions.

- Help text: add audit subcommands
  • print_usage(): include `vizier audit ...` commands if implemented by the audit TODO; otherwise, hide behind a feature flag `audit`.

Acceptance addition:
- Commits created by --save include an Audit-Anchor trailer when audit code is compiled in; otherwise behavior unchanged.

---

