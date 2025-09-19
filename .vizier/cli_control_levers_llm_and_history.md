---
Update (2025-09-13):
- Fold in native chat + diff/edit UX requirement; these levers must be visible/controllable from TUI chat panes as well as CLI flags.
- Acceptance expanded to include: (a) TUI shows current LLM session params in chat header; (b) From chat, user can toggle confirm_destructive and history_limit for the active session; (c) Reversions can be initiated from a diff view.
- Pointers: vizier-tui/src/chat.rs (chat header + controls), vizier-core/src/history.rs (API), vizier-core/src/config.rs (live session overrides).

---


---
Update (bug fix context): Conversation commits must not include unrelated staged changes.

Behavioral guardrail to codify (acceptance addition):
- When the system records a conversation commit, any pre-existing staged changes unrelated to .vizier are preserved but excluded from that commit, then restored exactly after.

Acceptance:
1) With arbitrary files staged (A/M/D/R), initiating a conversation write creates a commit that touches only .vizier paths; previously staged changes remain staged afterward and are not part of that commit.
2) Rename scenarios and mixed A/M/D staged sets are preserved across the conversation commit.

Pointers: vizier-core/src/auditor.rs (conversation commit flow), vizier-core/src/vcs.rs (stage/unstage/snapshot_staged/restore_staged tests).

Thread: Operation history + reversibility; Narrative contract + drift guardrails.


---


---
Update (2025-09-15): CLI refactor + commit isolation guardrail landed

- CLI refactor: Responsibility for finding the project root moved to vizier-core auditor; CLI now calls auditor::find_project_root(). Keep provider_arg_to_enum in actions.rs for now.
- Commit isolation (behavioral guardrail): Conversation/TODO commits must exclude unrelated staged changes and preserve the staged set exactly after the commit.

Acceptance additions:
1) With arbitrary staged changes (A/M/D/R), a conversation commit modifies only .vizier paths; previously staged changes remain staged and untouched after.
2) Rename scenarios and mixed staged sets are preserved across the conversation commit.

Pointers: vizier-core/src/auditor.rs (conversation commit flow), vizier-core/src/vcs.rs (stage/unstage/snapshot_staged/restore_staged + tests), vizier-cli/src/main.rs (auditor::find_project_root).

Thread: Operation history + reversibility; Narrative contract + drift guardrails.


---

---
Update (2025-09-19): Commit gating + confirmation flows (TUI + CLI)

Tension: We need explicit commit gates so AI-proposed changes don’t land without operator intent. Two modes emerge:
- TUI: in-chat editing of commit message/narrative change proposal with confirm/apply affordances.
- CLI: uncertainty about UX; likely a file-based edit/ingest flow for headless environments.

Behavioral goals:
- Any write-producing operation pauses at a "Pending Commit" gate showing:
  - Proposed diff scope and a commit message/narrative text the operator can edit.
  - Clear actions: Confirm & Commit, Amend Message, Reject.
- TUI specifics:
  - Chat split-view shows proposed diff and an editable commit message panel.
  - Keyboard shortcuts/buttons: (A)ccept, (E)dit message, (R)eject; status line shows confirmation requirement derived from config.confirm_destructive/auto_commit.
  - Acceptance: Accept applies exactly the shown hunks; message becomes the VCS commit message; history records the operation; Revert is available afterward.
- CLI specifics:
  - When interactive: open $EDITOR on a temporary commit proposal file containing:
    - Header with instructions + current config levers in effect
    - Proposed commit message (editable)
    - Separator
    - Diff (read-only section, edits ignored on ingest)
  - On save/exit: tool ingests the edited message; if file saved empty or contains "# abort" → abort without committing.
  - When non-interactive or --confirm supplied: allow providing a message via --message or via a path --message-file; otherwise refuse to commit unless --yes is given and config allows non-interactive writes.

Acceptance criteria:
1) With confirm gates on, TUI presents an editable commit message next to the diff, and requires explicit Accept before any files change on disk. Reject leaves workspace untouched.
2) After Accept in TUI, the exact displayed hunks are applied; the commit message equals the edited text; operation recorded; revert restores pre-op state.
3) CLI interactive: running `vizier apply` launches $EDITOR on a proposal file; on save, commit uses edited message; on empty save or `# abort`, command exits 0 with "aborted" and no changes.
4) CLI non-interactive: `vizier apply --yes --message "..."` commits without editor; absence of `--yes` or message causes a refusal with guidance, unless config.auto_commit=true and confirm gates allow it.
5) In headless CI mode, commands fail fast unless an explicit allowlist flag is set; no editor is spawned.

Pointers: vizier-tui/src/chat.rs (diff/editor panels, accept/reject), vizier-cli/src/main.rs (flag parsing, editor launch), vizier-core/src/auditor.rs + vcs.rs (commit boundaries), vizier-core/src/history.rs (record/revert), vizier-core/src/config.rs (confirm_destructive, auto_commit, non_interactive_mode).

Threads: CLI/TUI surface area; Operation history + reversibility; Headless discipline.


---

