# Vizier

**The narrative maintainer for software development — where code changes become story arcs, and every commit advances the plot.**

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
- **Pending Commit Gate**: Review diffs with split-view TUI; accept/reject changes before they land
- **Reversible Operations**: Every accepted change can be reverted; prefer patches, fall back to VCS
- **Configurable Guardrails**: Control destructive operations, auto-commit behavior, thinking depth

### Development Workflow Integration
- **Git-Native**: All changes are commits; conversation transcripts embed in empty commits for reconstruction
- **Terminal-First**: Chat TUI with diff view, modal navigation, long message handling, and scrolling
- **LLM-Augmented**: Multiple provider support (OpenAI/Anthropic), configurable prompts, thinking modes
- **Repository Bootstrap**: Analyze existing codebases to generate initial snapshot and seed threads

### Operational Tools
- **File-Aware Context**: Ignore-aware tree walking, behavioral diff analysis, cross-repository TODO management
- **Audit Trail**: Token counting, session recording, transcript commits with conversation hashes
- **Extensible Prompts**: System prompt customization via drop-in files or CLI flags; visible in meta
- **Issue Bridge**: Connect to GitHub Issues for task tracking while preserving narrative focus

## Usage

### Quick Start

```bash
# One-shot: Convert a request into TODOs and snapshot updates
vizier ask "add retry logic to the API client"

# Interactive: Launch the chat TUI for ongoing conversation
vizier chat

# Save: Commit your work with AI-generated conventional messages
vizier save              # commits HEAD changes
vizier save HEAD~3..HEAD # commits specific range
```

### Core Commands

#### Conversation & Editing
- `vizier ask <message>` — Single-shot request; updates TODOs/snapshot and exits
- `vizier chat` — Interactive TUI with split diff view and narrative maintenance

#### Snapshot Management
- `vizier snapshot init` — Bootstrap `.vizier/.snapshot` from repository analysis
  - `--depth N` — Limit Git history scan
  - `--paths <glob>` — Restrict analysis scope
  - `--issues github` — Enrich with external issue tracking

#### Commit Workflow
- `vizier save [REV]` — The "save button" for your work:
  1. Updates snapshot/TODOs based on changes
  2. Creates conversation commit (full transcript)
  3. Creates .vizier commit (narrative changes)
  4. Creates code commit (with AI-generated message)

  Options:
  - `-m <note>` — Add developer note to commit
  - `--no-conversation` — Skip conversation commit
  - `--no-code` — Skip code commit

#### TODO Maintenance
- `vizier clean <filter>` — Revise/remove TODOs; use `"*"` for all
  - Deduplicates threads
  - Removes completed work
  - Ensures evidence-based tasks

### Configuration

Configure via CLI flags or config file:

```bash
# Use specific model
vizier ask "..." -p anthropic

# Override system prompt
vizier chat --system-prompt-override ./prompts/custom.md

# Set thinking level
vizier ask "..." --thinking-level deep

# Non-interactive mode
vizier save --yes --commit-message "feat: add retry logic"
```

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
- **Chat TUI**: Streaming conversations with diff preview
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