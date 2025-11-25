# Vizier

**A managed VCS harness for using agents.**

Vizier is the control plane between your agents and your Git repository. It's not an AI assistant — it's the infrastructure that lets you safely use *your* agents (Claude Code, Cursor, custom scripts) without letting them write directly to your repo.

**Bring Your Own Agent.** Vizier doesn't care which agent you use. It cares *how* that agent interacts with version control: isolated branches, audited commits, narrative tracking, and compliance gates.

## Why You'd Use It

Use Vizier when you need guardrails for AI-assisted development without outsourcing your repo to another SaaS surface.

**You'd use Vizier if:**
- You want agents to propose edits, but raw writes into your repo are unsafe
- Your commits explain what changed but not the ongoing narrative or rationale
- Compliance teams expect architecture docs, review records, and traceable AI usage
- You prefer a repo-local control plane that travels with Git history

**You might skip Vizier if:**
- You're comfortable with agents writing directly to your working tree
- You don't need agent coordination, just basic prompting
- Commit messages + PR descriptions are enough documentation for your team

## Getting Started

**Prerequisites**: Rust toolchain (for building from source), Git 2.x+

```bash
# Install from source
cargo install vizier

# Initialize in your repo
cd your-project
vizier init-snapshot

# Update the narrative
vizier ask "add retry logic to the API client"

# Commit narrative changes alongside code
vim src/client.rs
vizier save -m "feat: add retry with exponential backoff"
```

**What you'll see:**
```
Outcome: Save complete
Files: src/client.rs (M), .vizier/.snapshot (M)
Session: .vizier/sessions/abc123/session.json
```

That's it. Vizier updates `.vizier/.snapshot` and TODO threads as you work. Every change is Git-tracked and auditable.

## What Vizier Does

**Coordinates agents safely** — The `draft → approve → review → merge` workflow keeps agent edits isolated on branches. Your working tree stays clean. Works with any agent backend.

**Maintains living documentation** — `.vizier/.snapshot` is your project's story bible. Threads and TODOs give every change narrative context, regardless of which agent wrote the code.

**Enforces guardrails** — Pending commit gates, review checks, and session logs mean every AI interaction is auditable and reversible. You control the VCS rules; agents follow them.

**Stays Git-native** — Every artifact is a branch, commit, or tracked file. No external services, no lock-in. Vizier is just Git with opinions about commit hygiene.

## Core Concepts

### Snapshot
A single-page document capturing what your code does and why it exists. Think of it as a diffable story bible that evolves with your project.

### Threads & TODOs
Threads are narrative arcs (e.g., "improve error handling"). TODOs are specific tasks that advance those threads, always tied back to the Snapshot with concrete acceptance criteria.

### Agent Control Plane
Agents never write directly to your repo. They work through Vizier commands that stage changes behind disposable worktrees, commit with audit trails, and surface outcomes for review.

**Vizier is backend-agnostic.** Use Claude Code, Cursor, Gemini, or a custom shell script. Vizier's only opinion is that agents communicate via its VCS interface, not by touching your working tree directly.

## Workflows

### Human-Driven: Ask and Save

Use Vizier to maintain narrative even without agents:

```bash
vizier ask "what's the status of the API refactor?"
# → Updates .vizier/.snapshot and TODO threads

# Make code changes
vim src/client.rs

vizier save -m "refactor: consolidate client interfaces"
# → Commits code + narrative together
```

**Example output:**
```
Outcome: Save complete
Files: src/client.rs (M), .vizier/.snapshot (M), .vizier/todo_api_refactor.md (M)
Commit: a1b2c3d
```

`vizier ask` updates Snapshot/TODOs. `vizier save` commits code + narrative together.

### Agent-Heavy: Draft, Approve, Review, Merge

Let agents implement features on isolated branches:

```bash
# 1. Create a plan on a draft branch
vizier draft "add rate limiting to API client"
# → Creates draft/rate-limiting branch with plan document

# 2. Implement the plan (agent-backed)
vizier approve rate-limiting
# → Applies changes in isolated worktree, commits to draft branch

# 3. Review and optionally apply fixes
vizier review rate-limiting
# → Runs checks, streams critique, offers to apply fixes

# 4. Merge to main with embedded plan
vizier merge rate-limiting
# → Squashes to two commits: implementation + merge with embedded plan
```

**Your checkout never changes until you merge.** Every step uses temporary worktrees to keep your working tree clean.

**Example output from `vizier merge`:**
```
Outcome: Merge complete
Plan: rate-limiting
Target: main
Commits: 2 (implementation + merge)
Branch draft/rate-limiting deleted
```

Need details on conflict resolution, CI/CD gates, or custom prompts? See `docs/workflows/draft-approve-merge.md`.

## Configuration

Vizier reads `.vizier/config.toml` for agent backends and workflow settings:

```toml
[agents.default]
backend = "agent"  # Pluggable: use any agent backend

[agents.review]
backend = "gemini"  # Mix and match per command

[review.checks]
commands = ["cargo test", "cargo clippy"]

[merge]
squash = true  # Clean two-commit history per plan
```

**Bring Your Own Agent means true flexibility:**
- Mix backends per command (e.g., Gemini for review, Claude Code for implementation)
- Point at custom shell scripts instead of bundled shims
- Define CI/CD gates that must pass before merging
- Customize prompts via `.vizier/config.toml` or standalone files

**Inspect before running:**
```bash
vizier plan --json  # See resolved settings without touching repo
```

**Configuration layers** (later wins):
1. Global defaults (`~/.config/vizier/config.toml`)
2. Repo overrides (`.vizier/config.toml`)
3. CLI flags (`--backend`, `--cicd-script`, etc.)

See `example-config.toml` for annotated examples, or `docs/config-reference.md` for the complete catalogue.

## Philosophy: Narrative Maintainer

Vizier treats software development as story editing, not just diff management.

- **Code is story** — The Snapshot is your single-frame story bible
- **Every TODO is a Chekhov's gun** — Specific, contextual, inevitable to resolve
- **Evidence beats speculation** — Ground changes in observable behavior and tests
- **Continuity beats churn** — Evolve existing threads instead of spawning duplicates
- **Operators stay in control** — Agents amplify intent but never free-drive
- **VCS hygiene over agent choice** — Vizier has strong opinions about Git interactions, zero opinions about which AI writes your code

## Additional Resources

**Essential reading:**
- `docs/workflows/draft-approve-merge.md` — Complete plan workflow guide (conflict resolution, CI/CD gates)
- `example-config.toml` — Annotated configuration examples you can copy

**Reference documentation:**
- `docs/config-reference.md` — Every knob, default, and override
- `docs/prompt-config-matrix.md` — Customizing agent instructions per command
- `.vizier/.snapshot` — The living narrative of this repo (meta!)

## Troubleshooting

**Agent not responding?**
```bash
vizier test-display --scope review  # Test backend with harmless prompt
vizier plan --json                  # Inspect resolved config
```

**Need to audit what happened?**
```bash
cat .vizier/sessions/<id>/session.json  # Full transcript + token usage
```

**Unexpected behavior?**
- Check `git status` — Vizier refuses to run with a dirty worktree for safety
- Review `.vizier/.snapshot` — The narrative might be stale
- Try `vizier help` or `vizier <command> --help` for detailed usage

**Common gotchas:**
- Config files are layered (global → repo → CLI flags); use `vizier plan` to see what wins
- Agent backends are pluggable scripts; bundled shims live under `share/vizier/agents/`

---

For questions, issues, or contributions, see `.vizier/.snapshot` for project status and `AGENTS.md` for agent integration notes.
