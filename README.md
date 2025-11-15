# Vizier

**The narrative maintainer for software development — where code changes become story arcs, and every commit advances the plot.**

### Who I Am
Repo‑native assistant that plans, edits, and audits with you in the loop. It understands your tree and turns conversations into TODOs and a living snapshot — fast, auditable, reversible.

### What I Can Do (today)
- Plan concrete steps from a high‑level goal
- Make scoped edits across files and keep diffs tidy
- Gate changes with an auditor/pending‑commit flow
- Explain decisions and current project state on demand

### How To Use Me (quickstart)
- Get help: `vizier help`
- One‑shot: `vizier ask "add retry logic to the API client"`
- Chat: `vizier chat`
- Save narrative + code changes: `vizier save`

### How Agents Can Talk To Me
Drive via CLI/stdio today; prefer structured outputs where available. Coming soon: stable protocol/JSON stream mode for tight interop. See `AGENTS.md`.

## Ethos

Software is a living narrative. Every codebase tells an evolving story about promises made, constraints discovered, and tensions resolved. Most development tools treat this story as exhaust — comments scattered through code, issues divorced from implementation, and commit messages that explain the "what" but forget the "why."

Vizier inverts this relationship. It treats the narrative as the primary artifact and code changes as plot points in an ongoing story. By maintaining a living snapshot of your project's trajectory and threading conversations through durable narrative arcs, Vizier ensures that every developer can understand not just what the code does, but why it exists and where it's headed.

### Core Principles

**Code as Story, Not Index**
Your codebase isn't a file tree with decorations. It's a narrative about behaviors, promises, and tensions. Vizier maintains a single-frame truth — the snapshot — that captures both the CODE STATE (what users can observe) and the NARRATIVE STATE (the themes and threads that explain why). No file-by-file inventories. No orphaned documentation. Just the story that matters.

**Operator Control, Not Automation**
AI doesn't drive; you do. Every change passes through explicit gates. Every edit is reversible. The snapshot is your story bible, Git remains the authority, and you decide what lands. Vizier amplifies your maintainer instincts without hijacking your repository. Think of it as a narrative co-pilot that never touches the controls without permission.

**Evidence Over Speculation**
Every TODO must resolve a real tension observable in behavior. No "investigate X" placeholders. No architectural astronautics. If a task can't be tied to existing code behavior, tests, or user reports, it isn't ready. Vizier uses tools to gather evidence, then writes plot points that feel inevitable once context is clear.

**Continuity Through Threading**
Ideas evolve. Requirements shift. Decisions cascade. Vizier doesn't scatter these changes across isolated tickets — it weaves them into narrative threads that persist across sessions. Each TODO references its thread. Each thread develops over time. Duplicate threads are plot holes. Evolution beats spawning twins.

## Capabilities

### Narrative Maintenance
- **Living Snapshot**: A single-page story bible that any developer can read to predict your next commit
- **Threaded TODOs**: Plot points that reference concrete code locations and develop ongoing narrative arcs
- **Conversation Memory**: Every discussion becomes part of the auditable history, embedded in Git

### Intelligent Gates & Controls
- **Commit Isolation**: Conversation changes never contaminate code commits; staged work is preserved
- **Pending Commit Gate**: Review and accept/reject changes before they land
- **Reversible Operations**: Every accepted change can be reverted; prefer patches, fall back to VCS
- **Configurable Guardrails**: Control destructive operations, auto-commit behavior, thinking depth

### Development Workflow Integration
- **Git-Native**: All changes are commits; transcripts live under `.vizier/sessions/<id>/session.json`, keeping Git history focused on narrative + code diffs only
- **Terminal-First**: Chat TUI for interactive conversations; line‑oriented CLI for scripting
- **LLM-Augmented**: Multiple provider support (OpenAI/Anthropic), configurable prompts, thinking modes
- **Repository Bootstrap**: Analyze existing codebases to generate initial snapshot and seed threads
- **Draft Reviews**: `vizier draft` spins up a temporary worktree, runs Codex, and commits `.vizier/implementation-plans/<slug>.md` on `draft/<slug>`; `vizier approve <plan>` reuses a disposable worktree to implement the approved plan via Codex (auto-staging and committing the branch); `vizier merge <plan>` refreshes `.vizier/.snapshot` inside a worktree and then lands the branch into the primary line with a plan-driven commit message

### Operational Tools
- **File-Aware Context**: Ignore-aware tree walking, behavioral diff analysis, cross-repository TODO management
- **Audit Trail**: Token counting plus repo-local session logs whose IDs are referenced from every narrative/code commit
- **Extensible Prompts**: System prompt customization via drop-in files or CLI flags; visible in meta
- **Issue Bridge**: Connect to GitHub Issues for task tracking while preserving narrative focus

## Usage

### Quick Start

```bash
# One-shot: Convert a request into TODOs and snapshot updates
vizier ask "add retry logic to the API client"
# or pull the prompt from a file
vizier ask --file specs/retry.md

# Interactive: Launch chat for ongoing conversation
vizier chat

# Save: Commit your work with AI-generated conventional messages
vizier save              # commits HEAD changes
vizier save HEAD~3..HEAD # commits specific range
```

### Core Commands

#### Conversation & Editing
- `vizier ask <message>` — Single-shot request; updates TODOs/snapshot and exits (use `--file PATH` to read the prompt from disk)
- `vizier chat` — Interactive TUI for conversational maintenance
- `vizier draft [--name SLUG] <spec>` — Spins up a temporary worktree at the primary branch tip, runs Codex to produce `.vizier/implementation-plans/<slug>.md`, commits it on `draft/<slug>`, and tells you which branch/plan file to inspect (Codex-only; your working tree stays untouched)
- `vizier approve <plan>` — Executes the approved implementation plan inside a disposable worktree (Codex-only), streaming Codex output to stderr, updating `.vizier`, and auto‑committing the draft branch so reviewers can diff `git diff <target>...<branch>`. Flags: `--list`, `-y/--yes`, `--target`, `--branch`. [Learn more](docs/workflows/draft-approve-merge.md#vizier-approve-implement-the-plan-safely)
- `vizier merge <plan>` — Runs the same narrative-refresh flow as `vizier save` inside a temporary worktree, removes `.vizier/implementation-plans/<plan>.md`, commits the `.vizier` edits on the plan branch, then merges `draft/<plan>` into the detected primary branch with a plan-driven commit message. On conflicts, Vizier writes a resume token under `.vizier/tmp/merge-conflicts/<plan>.json`; resolve the files (or rerun with `--auto-resolve-conflicts` to let Codex try first) and invoke `vizier merge <plan>` again to finish the merge. Flags: `-y/--yes`, `--delete-branch`, `--target`, `--branch`, `--note`, `--auto-resolve-conflicts`. [Learn more](docs/workflows/draft-approve-merge.md#vizier-merge-land-the-plan-with-metadata)

#### Documentation Prompts
- `vizier docs prompt <scope>` — Emit or scaffold architecture templates described in `PROMPTING.md`
  - `--write PATH` writes the template to a file (use `-` to force stdout)
  - `--scaffold` materializes the template under `.vizier/docs/prompting/`
  - `--force` overwrites existing files when used with `--write` or `--scaffold`
  - Scopes: `architecture-overview`, `subsystem-detail`, `interface-summary`, `invariant-capture`, `operational-thread`

#### Snapshot Management
- `vizier snapshot init` — Bootstrap `.vizier/.snapshot` from repository analysis
  - `--depth N` — Limit Git history scan
  - `--paths <glob>` — Restrict analysis scope
  - `--issues github` — Enrich with external issue tracking

#### Commit Workflow
- `vizier save [REV]` — The "save button" for your work:
  1. Updates snapshot/TODOs based on changes
  2. Writes the session transcript + metadata to `.vizier/sessions/<session_id>/session.json`
  3. Creates .vizier commit (narrative changes)
  4. Creates code commit (with AI-generated message)

  Options:
  - `-m <note>` — Add developer note to commit
  - `--no-code` — Skip code commit

#### TODO Maintenance
- `vizier clean <filter>` — Revise/remove TODOs; use `"*"` for all
  - Deduplicates threads
  - Removes completed work
  - Ensures evidence-based tasks

### Configuration

Configure via CLI flags or config file:

If no path is provided, Vizier will look for `~/.config/vizier/config.toml` (TOML or JSON) and fall back to its built-in defaults when that file is missing.

```bash
# Use specific model
vizier ask "..." -p anthropic

# Override system prompt
vizier chat --system-prompt-override ./prompts/custom.md

# Set thinking level
vizier ask "..." --reasoning-effort high

# Non-interactive commit message
vizier save -m "feat: add retry logic"
```

#### Backend selection

Vizier can operate in either the legacy HTTP (`wire`) backend or the Codex backend that edits the workspace directly. The global config defaults to Codex and automatically falls back to the wire stack when Codex cannot produce a response. You can tune the behavior in `~/.config/vizier/config.toml`:

```toml
backend = "codex"            # or "wire"
fallback_backend = "wire"    # retry with wire if Codex exits early

[codex]
binary = "/usr/local/bin/codex"          # defaults to resolving `codex` on $PATH
profile = "vizier"                       # pass an empty string to unset
bounds_prompt = ".vizier/codex-bounds.md" # optional override for the bounds text
extra_args = ["--log-json"]              # forwarded to `codex exec`
```

Per-command overrides are available:

- `--backend codex|wire` selects the backend for a single invocation.
- `--codex-bin PATH`, `--codex-profile NAME`, and `--codex-bounds-prompt PATH` mirror the config keys above.
- `-p/--model` applies only to the wire backend; the flag is ignored (with a warning) when Codex is active.

When Codex runs, it edits `.vizier/.snapshot` and TODO files in-place and streams progress events through the CLI. The usual commit gates (`vizier save`, staged hunks, etc.) still apply, and token usage is reported when the backend shares it. If Codex omits usage numbers, Vizier reports them as `unknown` instead of showing stale totals.

## Architecture

Vizier follows a workspace structure with clear separation of concerns:

### vizier-core
The narrative engine and tool system:
- **Prompts**: System prompts encoding the story-editor philosophy
- **Tools**: LLM-exposed functions for TODO/snapshot manipulation
- **Auditor**: Complete session capture and token accounting
- **VCS Integration**: Commit isolation, stage/restore, diff generation
- **File Tracking**: `.vizier/` change batching and commits

### vizier-cli
Command-line interface and workflow orchestration:
- **Action Handlers**: Save flows, bootstrap logic, TODO cleaning
- **Provider Management**: Multi-LLM support with runtime switching
- **Config System**: Hierarchical settings (CLI > session > profile > default)

### Terminal UI
Interactive interfaces for narrative work:
- **Chat TUI**: Streaming conversations
- **Modal Navigation**: View mode (safe browsing) vs Edit mode
- **Editor Integration**: `$EDITOR` launching for detailed edits

## The Maintainer Mindset

Vizier embodies a fundamental shift in how we think about development tools:

**You're not a transcriptionist** documenting what happened. You're a story editor surfacing themes and reducing noise.

**Every TODO is a scene** serving the larger narrative. It should feel inevitable once context is clear, like Chekhov's gun that must go off.

**Responses do work**, they don't describe work. When Vizier updates your snapshot, it's not planning to do it later — it's done.

**Evidence beats speculation**. Tie every change to observable behavior, test results, or user reports. The codebase is the source of truth; the narrative explains why that truth exists.

This isn't about making Git "smart" or replacing your workflow with AI. It's about recognizing that software development is fundamentally about maintaining narratives — the story of what your code promises, why those promises exist, and how they evolve. Vizier just makes that story explicit, auditable, and impossible to lose.

## Build & Install

```bash
# Clone and build
git clone https://github.com/your-org/vizier.git
cd vizier
cargo build --release

# Or install directly
cargo install vizier

# Initialize in your project
cd your-project
vizier snapshot init
```

**Requirements**: Rust toolchain, Git repository, OpenAI/Anthropic API access

## License

MIT
