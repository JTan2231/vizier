# Vizier

A terminal-native, LLM-assisted project steward. Vizier turns natural-language intent into concrete, code-anchored TODOs, maintains a living “snapshot” of project direction, and records every conversation and change as auditable git history.

---

## Philosophy

* **Code as narrative.** A codebase tells a story about promises and constraints. Vizier treats TODOs as plot points and the snapshot as the story bible. Tasks must resolve real tensions in the code—not decorate them.
* **Context before action.** The model receives a file-tree, current TODOs, and diffs. If a task can’t be tied to existing code, it isn’t ready. No “investigate X” placeholders.
* **Maintainer mindset.** Responses do work. The assistant updates TODOs/snapshot first; explanation is secondary. Duplicate TODOs are plot holes; Vizier develops existing threads instead of spawning parallel ones.
* **Auditability is product, not paperwork.** Every session’s conversation and every `.vizier/` change can be committed—reconstructable after the fact. LLM involvement increases traceability, not ambiguity.

---

## Product focus

* **Inputs**

  * Free-form user message (CLI or chat TUI)
  * Repository context: file tree, existing `.vizier/` TODOs, optional `git diff`
  * Provider selection (`-p openai|anthropic`)

* **Outputs**

  * Updated `.vizier/` state:

    * `todo_*.md` files (markdown)
    * `.snapshot` (project trajectory)
  * Optional code commit with AI-generated, conventional message
  * Optional “conversation commit” embedding the session transcript

* **Contracts**

  * Each TODO references concrete files/locations or clearly scoped modules.
  * No research tasks; Vizier uses tools to gather context and writes actionable items.
  * Commits separate **conversation** from **code** and **.vizier** updates for clean history.

* **Boundaries**

  * Focused on terminal workflows and git repositories.
  * Not a live editor; it appends/updates TODO files and snapshot, and helps you commit.

---

## Architecture (workspace)

* **`cli`** — entrypoint, argument parsing, provider config, auditing/commits.

  * `auditor.rs` captures every LLM exchange, tallies tokens, writes a transcript commit, and (if needed) commits `.vizier/` changes.
  * `main.rs` wires commands, builds system prompt, calls tools, and manages save flows.

* **`prompts`** — prompt system and project tools.

  * `SYSTEM_PROMPT_BASE` encodes the “story-editor” rules.
  * `tools.rs` exposes functions to the model:

    * `diff()`
    * `add_todo()`, `update_todo()`, `delete_todo()`, `list_todos()`, `read_todo()`
    * `read_snapshot()`, `update_snapshot()`
  * `file_tracking.rs` tracks writes inside `.vizier/` and batches their commit.
  * `tree.rs`, `walker.rs` build a lightweight, ignore-aware view of the repo.

* **`tui`** — terminal UIs.

  * **List TUI**: browse/edit `.vizier/` items and snapshot (opens `$EDITOR` for files).
  * **Chat TUI**: stream conversations with status updates and token counters.

---

## Workflow

1. **Context build** — Vizier assembles a prompt with file tree, `.vizier/` contents, working directory, and (when saving) a diff.
2. **Action** — The model uses tools to append/update `todo_*.md` and `.snapshot`, grounded in real files.
3. **Audit** — The `Auditor` can:

   * Create a **conversation commit** (empty commit with the full transcript).
   * Create a **.vizier commit** summarizing changes (message derived from the diff).
   * Optionally create a **code commit** (excluding `.vizier/`) with an AI-generated conventional message, plus your author note if provided.

---

## Commands (CLI)

```
vizier [OPTIONS] [MESSAGE]
```

* `-c, --chat` — interactive chat session.
* `-l, --list` — browse `.vizier/` TODOs and snapshot in a TUI.
* `-s, --save <REF|RANGE>` — “save button”:

  * Updates snapshot/TODOs via tools,
  * Commits conversation and `.vizier/` changes,
  * Generates a conventional commit for code based on `git diff <REF|RANGE>` (excludes `.vizier/`).
* `-S, --save-latest` — like `-s HEAD`.
* `-m, --commit-message <MSG>` — append an author note to the code commit (mutually exclusive with `-M`).
* `-M, --commit-message-editor` — open `$EDITOR` to author the note (mutually exclusive with `-m`).
* `-p, --provider <NAME>` — select LLM provider (`openai`, `anthropic`).
* `-f, --force-action` — hint that the agent should perform an action.
* Standard `-h/--help`, `-V/--version`.

**Positional `[MESSAGE]`**: one-shot request to generate/update TODOs/snapshot via tools.

---

## Auditing & commits (behavior)

* **Conversation commit**

  * Empty commit whose message embeds the full user/assistant transcript (non-tool messages).
  * Serves as an immutable anchor (`rev-parse` recorded) for later references.

* **`.vizier/` commit**

  * Stages `.vizier/` changes and commits them with an LLM-generated summary referencing the conversation hash.
  * File tracking is internal; only created when `.vizier/` actually changed.

* **Code commit**

  * Uses `git diff` of your specified ref/range (or `HEAD` for `--save-latest`), excluding `.vizier/`.
  * Message follows conventional-commit structure produced by a dedicated commit-writer prompt.
  * Optional author note is prefixed and the conversation hash is included for traceability.

---

## `.vizier/` data model

* `todo_*.md` — Markdown TODOs; `add_todo(name, description)` names the file, `update_todo(todo_name, update)` appends with separators.
* `.snapshot` — Overwritten by `update_snapshot(content)`; represents current project trajectory.
* Tools operate with fuzzy path matching when reading files for convenience; creation and updates target `.vizier/` explicitly.

---

## Build & run

```bash
# Build
cargo build --release

# One-shot message
target/release/vizier "turn TODOs in code comments into actionable items"

# Chat
target/release/vizier --chat

# Browse project TODOs/snapshot
target/release/vizier --list

# Save latest work into auditable history and generate a code commit
target/release/vizier --save-latest -m "cleanup parser edge cases"
```

**Requirements:** recent Rust toolchain; run inside a git repository.

---

## Notes on provider & tokens

* Provider defaults are configurable; `-p` switches between supported backends at runtime.
* The auditor accumulates prompt/completion token counts across the session and displays totals on completion.

---

## License

MIT
