# Vizier

**A managed VCS harness for using agents.**

Vizier is the control plane between your agents and your Git repository. It's not an AI assistant — it's the infrastructure that lets you safely use *your* agents (Claude Code, Cursor, custom scripts) without letting them write directly to your repo.

**Bring Your Own Agent.** Vizier doesn't care which agent you use. It cares *how* that agent interacts with version control: isolated branches, audited commits, narrative tracking, and compliance gates.

## What Vizier Does (At a Glance)
- Coordinates agents through the `draft → approve → review → merge` workflow atop Git without touching your working tree.
- Maintains a living Snapshot plus supporting narrative docs so every change advances a named thread.
- Enforces guardrails: pending commit gates, review artifacts, architecture-doc citations, and repo-local evidence under `.vizier/`.
- Keeps workspaces clean via temporary worktrees and repo-level session logs.
- Operates 100% Git-native — every artifact is a branch, commit, or tracked file you can inspect locally.

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
A single-page story bible that captures both CODE STATE (observable behaviors/interfaces) and NARRATIVE STATE (themes, tensions, open threads). Treat it like a diffable frame of truth.

- Always read-before-write; prefer minimal, diff-like updates over rewrites.
- Cross-link snapshot paragraphs to the narrative docs they inform.
- Ground every edit in evidence from the codebase or user behavior — never invent internals.

### Narrative docs & threads
Threads are narrative arcs; narrative docs (under `.vizier/narrative/threads/`) are the Chekhov's guns that resolve each tension.
Legacy `.vizier/todo_*.md` files are no longer read; migrate any remaining notes into `.vizier/narrative/threads/` so they stay in the prompt context.

- Every narrative doc advances a named thread and cites the relevant slice of the Snapshot.
- Tasks stay product-level by default; reach for implementation detail only when safety or explicit requests demand it.
- No "investigate X" placeholders — each narrative doc commits to an observable outcome with acceptance criteria.

### Agent Control Plane
Agents never write directly to your repo. They work through Vizier commands that stage changes behind disposable worktrees, commit with audit trails, and surface outcomes for review.

**Vizier is backend-agnostic.** Use Claude Code, Cursor, Gemini, or a custom shell script. Vizier's only opinion is that agents communicate via its VCS interface, not by touching your working tree directly.

## Workflows

### Human-Driven: Ask and Save

Use Vizier to maintain narrative even without agents:

- `vizier ask "..."` captures a directive, updates the snapshot/narrative docs, and exits with a concise outcome.
- Vizier applies the default-action posture: unless you opt out, conversations update the narrative artifacts automatically.
- `vizier save` stages `.vizier/` edits plus code changes, runs the Auditor, and lands commits without disturbing existing staged work.

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

Each Vizier action lands a single commit that bundles code edits with canonical narrative assets (`.vizier/.snapshot` plus notes under `.vizier/narrative/threads/`). Plan documents, `.vizier/tmp/*`, and session logs stay as scratch artifacts and are filtered out automatically. Let agents implement features on isolated branches:

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

### Background jobs

Long-running commands can detach so your terminal stays free:

```bash
vizier draft --background "add rate limiting to API client"
vizier approve --yes --background rate-limiting
```

- Background is supported for `ask`, `draft`, `approve`, `review`, `merge`, and `save`. Destructive prompts still require `--yes` (or `--review-only` for reviews).
- Logs live under `.vizier/jobs/<id>/{stdout.log,stderr.log,job.json,outcome.json}` with session/outcome pointers and config/agent metadata.
- Inspect and manage jobs with `vizier jobs list|status|show|tail|attach|cancel|gc`.
- `[workflow.background]` controls allow/deny plus default quiet/progress (`--no-ansi`/`--no-pager` are forced for background runs).

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

**Configuration essentials:**
- `docs/config-reference.md` is the exhaustive list of configuration keys (scope, defaults, precedence, CLI overrides) plus deprecated entries; `docs/prompt-config-matrix.md` maps scopes to prompt kinds.
- `.vizier/config.toml` defines agent scopes (`[agents.ask]`, `[agents.save]`, `[agents.draft]`, `[agents.approve]`, `[agents.review]`, `[agents.merge]`), merge defaults (e.g., `[merge] squash = true` to keep two commits per plan, `[merge] squash_mainline = 2` for merge-heavy plan branches), backend options, and the prompt profiles attached to each command (`[agents.<scope>.prompts.<kind>]` with inline text or `path` overrides).
- `vizier plan` prints the fully resolved configuration (global + repo + CLI overrides) with per-scope backend/runtime selection; pass `--json` for a structured view. Help output pages on TTY by default; use `$VIZIER_PAGER` or `--pager`/`--no-pager` to control it. Quiet/`--no-ansi`/non-TTY fall back to plain stdout.
- If you do not pass `--config-file`, Vizier loads global config from `$XDG_CONFIG_HOME`/`$VIZIER_CONFIG_DIR` (when present) and overlays `.vizier/config.toml` so repo settings override while missing keys inherit your personal defaults. `VIZIER_CONFIG_FILE` is consulted only when neither config file exists.
- Agent backends run through shell scripts that stream JSON on stdout while Vizier handles the rest: pick a bundled shim via `agent.label` (`codex`/`gemini`, installed under `share/vizier/agents/`) or point `[agents.<scope>.agent].command` at your own script; tune `[agent].output` and `[agent].progress_filter` as needed. Each scope names a single backend; failures abort instead of falling back automatically.
- `.vizier/*.md` prompt files (IMPLEMENTATION_PLAN_PROMPT, REVIEW_PROMPT, MERGE_CONFLICT_PROMPT, etc.; legacy BASE_SYSTEM_PROMPT still works) remain the fallback when no scope-specific profile is defined. Per-scope documentation controls live under `[agents.<scope>.documentation]` (`enabled`, `include_snapshot`, `include_narrative_docs`) so you can slim prompts when needed.
- `.vizier/COMMIT_PROMPT.md` (or `[prompts.commit]`) replaces the default commit template. `example-config.toml` documents every knob, precedence rule (`CLI → scoped agent → default agent → legacy`), and shows how per-scope prompt profiles control backend selection and prompt text.

## Philosophy: Narrative Maintainer

Vizier treats software development as story editing, not just diff management.

- Code is story, not an index; the Snapshot is the single-frame story bible.
- Every narrative doc/TODO is a Chekhov's gun — specific, contextual, and inevitable to resolve.
- Evidence beats speculation; ground changes in observable behavior and tests.
- Continuity beats churn; evolve existing threads instead of spawning duplicates.
- Operators stay in control; agents amplify maintainer intent but never free-drive.
- VCS hygiene over agent choice — Vizier has strong opinions about Git interactions, zero opinions about which AI writes your code.

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
