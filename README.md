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
- Save narrative + code changes: `vizier save`
- Install completions: `vizier completions zsh >> ~/.zshrc` (or `source <(vizier completions bash)` for Bash, `vizier completions fish | source` for Fish) so Tab lists pending plan slugs for `vizier approve`/`vizier merge`

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
- **Terminal-First**: Line-oriented CLI flows (`vizier ask`, `vizier draft/approve/merge`, `vizier save`) — no alt-screen surfaces
- **LLM-Augmented**: Multiple provider support (OpenAI/Anthropic), configurable prompts, thinking modes
- **Repository Bootstrap**: Analyze existing codebases to generate initial snapshot and seed threads
- **Plan Workflow**: `vizier draft` spins up a temporary worktree, runs Codex, and commits `.vizier/implementation-plans/<slug>.md` on `draft/<slug>`; `vizier approve <plan>` reuses a disposable worktree to implement the approved plan via Codex (auto-staging and committing the branch); `vizier review <plan>` runs the configured checks, saves `.vizier/reviews/<slug>.md`, and optionally applies fixes; `vizier merge <plan>` refreshes `.vizier/.snapshot` inside a worktree and then lands the branch into the primary line with a plan-driven commit message

### Operational Tools
- **File-Aware Context**: Ignore-aware tree walking, behavioral diff analysis, cross-repository TODO management
- **Audit Trail**: Token counting plus repo-local session logs whose IDs are referenced from every narrative/code commit
- **Extensible Prompts**: Drop-in `.vizier/BASE_SYSTEM_PROMPT.md`, `.vizier/COMMIT_PROMPT.md`, `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`, `.vizier/REVIEW_PROMPT.md`, or `.vizier/MERGE_CONFLICT_PROMPT.md` (or set `[prompts.base|commit|implementation_plan|review|merge_conflict]` in config) to steer CLI editing plus the draft/approve/review/merge choreography.
- **Issue Bridge**: Connect to GitHub Issues for task tracking while preserving narrative focus

## Usage

### Quick Start

```bash
# One-shot: Convert a request into TODOs and snapshot updates
vizier ask "add retry logic to the API client"
# or pull the prompt from a file
vizier ask --file specs/retry.md

# Draft: Capture a Codex-generated plan branch from a spec file
vizier draft --file specs/retry.md --name retry-plan

# Save: Commit your work with AI-generated conventional messages
vizier save              # commits HEAD changes
vizier save HEAD~3..HEAD # commits specific range
```

### Core Commands

#### Conversation & Editing
- `vizier ask <message>` — Single-shot request; updates TODOs/snapshot and exits (use `--file PATH` to read the prompt from disk)
- `vizier draft [--name SLUG] <spec>` — Spins up a temporary worktree at the primary branch tip, runs Codex to produce `.vizier/implementation-plans/<slug>.md`, commits it on `draft/<slug>`, and tells you which branch/plan file to inspect (Codex-only; your working tree stays untouched)
- `vizier list [--target BRANCH]` — Lists each `draft/<slug>` branch that is ahead of the target branch (defaults to the detected primary) with the stored metadata so you can decide what to approve or merge next.
- `vizier approve <plan>` — Executes the approved implementation plan inside a disposable worktree (Codex-only), streaming Codex output to stderr, updating `.vizier`, and auto‑committing the draft branch so reviewers can diff `git diff <target>...<branch>`. Flags: `-y/--yes`, `--target`, `--branch`. Use `vizier list` beforehand if you need to see the pending plan backlog. Once you’ve sourced `vizier completions --shell <shell>`, the positional `plan` argument tab-completes against outstanding draft slugs. [Learn more](docs/workflows/draft-approve-merge.md#vizier-approve-implement-the-plan-safely)
- `vizier review <plan>` — Runs the configured review checks (`cargo check/test` by default when a `Cargo.toml` is present or the commands configured under `[review.checks]`), captures the diff summary, generates a Codex critique at `.vizier/reviews/<plan>.md`, updates the plan status (e.g., `review-ready`), and optionally applies fixes on the plan branch. Flags: `-y/--yes`, `--target`, `--branch`, `--review-only` (skip the fix prompt), `--skip-checks` (only generate the critique). [Learn more](docs/workflows/draft-approve-merge.md#vizier-review-critique-the-plan-branch)
- `vizier merge <plan>` — Runs the same narrative-refresh flow as `vizier save` inside a temporary worktree, removes `.vizier/implementation-plans/<plan>.md`, commits the `.vizier` edits on the plan branch, then merges `draft/<plan>` into the detected primary branch with a plan-driven commit message. After the merge commit is staged, Vizier runs the optional CI/CD gate script (`[merge.cicd_gate]` or `--cicd-script`). Successful runs print the merge summary (and delete `draft/<plan>` unless `--keep-branch` is set); failures stream the script’s stdout/stderr and abort before deleting/pushing so you can inspect the repo. `--auto-cicd-fix/--no-auto-cicd-fix` toggle Codex-backed remediation of gate failures (requires `[agents.merge]` on Codex), and `--cicd-retries` caps how many fix attempts Vizier makes before giving up. On conflicts, Vizier writes a resume token under `.vizier/tmp/merge-conflicts/<plan>.json`; resolve the files (or rerun with `--auto-resolve-conflicts` to let Codex try first) and invoke `vizier merge <plan> --complete-conflict` to finish the merge once the index is clean. Flags: `-y/--yes`, `--keep-branch`, `--target`, `--branch`, `--note`, `--auto-resolve-conflicts`, `--complete-conflict`, `--cicd-script PATH`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, `--cicd-retries N`. [Learn more](docs/workflows/draft-approve-merge.md#vizier-merge-land-the-plan-with-metadata)

#### Shell Ergonomics
- `vizier completions --shell <bash|zsh|fish|elvish|powershell>` — Emits a dynamic completion script that wires your shell’s Tab key to a hidden `vizier __complete` endpoint. Source it (`echo "source <(vizier completions --shell zsh)" >> ~/.zshrc`, `vizier completions --shell bash > ~/.config/vizier/completions && source ~/.config/vizier/completions`, `vizier completions --shell fish | source`, etc.) to get subcommand/flag completion plus plan-slug suggestions for `vizier approve`/`vizier review`/`vizier merge`.

#### Snapshot Management
- `vizier init-snapshot` — Bootstrap `.vizier/.snapshot` from repository analysis
  - `--depth N` — Limit Git history scan
  - `--paths <glob>` — Restrict analysis scope
  - `--issues github` — Enrich with external issue tracking

> Architecture docs will be scaffolded through the forthcoming compliance gate (see `.vizier/todo_architecture_doc_gate_and_commit_history.md`); until then copy the templates under `.vizier/docs/prompting/` or follow your org’s SOP.

#### Commit Workflow
- `vizier save [REV]` — The "save button" for your work:
  1. Updates snapshot/TODOs based on changes
  2. Writes the session transcript + metadata to `.vizier/sessions/<session_id>/session.json`
  3. Creates .vizier commit (narrative changes)
  4. Creates code commit (with AI-generated message)

  Options:
  - `-m <note>` — Add developer note to commit
  - `--no-code` — Skip code commit

_Default-Action Posture plus the TODO GC work tracked in `.vizier/todo_todo_todo_garbage_collection_on_save.md` now handle housekeeping; the retired `vizier clean` shim no longer exists._

### Configuration

Configure via CLI flags or config file:

If no path is provided, Vizier will look for `~/.config/vizier/config.toml` (TOML or JSON) and fall back to its built-in defaults when that file is missing.

For a complete reference of every configuration knob, keep `example-config.toml` in this repo handy; it documents each section and serves as the authoritative sample for what Vizier understands today.

```bash
# Use specific model
vizier ask "..." -p anthropic

# Override system prompt
vizier ask --system-prompt-override ./prompts/custom.md

# Set thinking level
vizier ask "..." --reasoning-effort high

# Non-interactive commit message
vizier save -m "feat: add retry logic"
```

#### Prompt overrides

Vizier loads its instructions from a prompt store. On startup it looks for the following drop-in files inside `.vizier/` and falls back to the baked-in defaults when a file is missing:

- `.vizier/BASE_SYSTEM_PROMPT.md`
- `.vizier/COMMIT_PROMPT.md`
- `.vizier/IMPLEMENTATION_PLAN_PROMPT.md`
- `.vizier/REVIEW_PROMPT.md`
- `.vizier/MERGE_CONFLICT_PROMPT.md`

Each template corresponds to the CLI flows that use it (ask/save, commit message drafting, plan generation, review critiques, and merge-conflict auto-resolution). You can also pin overrides via the config file:

```toml
[prompts]
base = "./prompts/base.md"
commit = "./prompts/commit.md"
implementation_plan = "./prompts/plan.md"
review = "./prompts/review.md"
merge_conflict = "./prompts/merge.md"
```

Changes are picked up the next time you launch `vizier`; restart between edits if you are iterating on wording.

#### Backend selection

Vizier can operate in either the legacy HTTP (`wire`) backend or the Codex backend that edits the workspace directly. The config now exposes scoped agent sections so you can keep most commands on Codex while pinning a subset (e.g., `vizier ask`) to wire. Declare defaults under `[agents.default]` and override individual commands with `[agents.ask]`, `[agents.save]`, `[agents.draft]`, `[agents.approve]`, `[agents.review]`, or `[agents.merge]`:

```toml
[agents.default]
backend = "codex"
fallback_backend = "wire"    # retry with wire if Codex exits early

[agents.ask]
backend = "wire"
model = "gpt-4.1"
reasoning_effort = "medium"

[agents.review.codex]
profile = "compliance"
bounds_prompt_path = "/work/policies/review_bounds.md"

[codex]
binary = "/usr/local/bin/codex"          # defaults to resolving `codex` on $PATH
extra_args = ["--log-json"]              # forwarded to `codex exec`
```

Precedence runs `CLI flags → [agents.<command>] → [agents.default] → legacy top-level keys`. CLI overrides remain the top lever: `--backend`, `--codex-bin/profile/bounds-prompt`, and `-p/--model` apply only to the command being executed, and they sit above any `[agents.*]` entries. Vizier warns when a model override is ignored because the resolved backend is Codex.

- `--backend codex|wire` selects the backend for the current command.
- `--codex-bin PATH`, `--codex-profile NAME`, and `--codex-bounds-prompt PATH` mirror the config keys and override `[agents.*.codex]` for that run.
- `-p/--model` and `-r/--reasoning-effort` apply only when the resolved backend is `wire`.

When Codex runs, it edits `.vizier/.snapshot` and TODO files in-place and streams progress events through the CLI. The usual commit gates (`vizier save`, staged hunks, etc.) still apply, and token usage is reported when the backend shares it. If Codex omits usage numbers, Vizier reports them as `unknown` instead of showing stale totals.

#### Merge CI/CD gate

The merge choreography can enforce a repo-defined quality gate before the draft branch lands. Configure a script plus remediation policy under `[merge.cicd_gate]`:

```toml
[merge.cicd_gate]
script = "./scripts/run-ci.sh"  # shell script executed from the repo root
auto_resolve = true             # let Codex attempt fixes when the script fails
retries = 2                     # number of Codex fix attempts before giving up
```

`vizier merge <slug>` (and `vizier merge <slug> --complete-conflict`) runs the script after staging the merge commit but before deleting the draft branch or pushing. A zero exit code finalizes the merge; a non-zero exit prints the collected stdout/stderr and aborts so you can investigate. With `auto_resolve = true`, Vizier prompts Codex to apply targeted fixes when the gate fails, rerunning the script up to `retries` times (Codex must be the resolved backend for `[agents.merge]`). CLI overrides — `--cicd-script PATH`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, and `--cicd-retries N` — provide per-run control without changing config files.

#### Codex progress history

Codex already emits a `codex exec --json` event stream; Vizier renders each event as a persistent `[codex] …` log line on stderr so you can see every phase instead of watching a spinner overwrite itself. Each line includes the phase/label, optional status + percentage, and any scoped detail/path pulled from the event’s `phase`, `label`, `message`, `detail`, `data.path`, `progress`, `status`, and `timestamp` fields. Quiet mode (`-q`) suppresses the lines, `-v` adds timestamps, and `-vv` appends the raw JSON payload for debugging. `vizier approve`, conflict auto-resolution, and all Codex-backed ask/save flows now share this renderer, so the history looks identical regardless of which command is running.

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
- **Action Handlers**: Save flows, bootstrap logic, and the forthcoming TODO GC wiring
- **Provider Management**: Multi-LLM support with runtime switching
- **Config System**: Hierarchical settings (CLI > session > profile > default)

### Terminal UX
Line-oriented CLI workflow with no alt-screen surfaces:
- **Ask**: `vizier ask` captures a directive, updates snapshot/TODOs, and exits.
- **Save**: `vizier save` is the gatekeeper — it stages conversation logs, `.vizier` edits, and code diffs with AI-authored commits.
- **Plan branches**: `vizier draft/approve/merge` orchestrate Codex-driven implementation behind disposable worktrees.
- **Editors**: When you need to edit commit messages, Vizier defers to `$EDITOR` rather than launching a custom interface.

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
vizier init-snapshot
```

**Requirements**: Rust toolchain, Git repository, OpenAI/Anthropic API access

## License

MIT
