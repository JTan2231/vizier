# Vizier

**The Git-native agent coordination and narrative layer for your repo.**

## Overview
Vizier lives inside your Git repository as the control plane between humans, agents, and history. It turns conversations into implementation plans, audited branches, and merge-ready commits while maintaining a living snapshot of your project's story — what the code does today and why it exists.

## What Vizier Does (At a Glance)
- Coordinates agents through the `draft → approve → review → merge` workflow atop Git without touching your working tree.
- Maintains a living Snapshot and threaded TODOs so every change advances a named narrative thread.
- Enforces guardrails: pending commit gates, review artifacts, architecture-doc citations, and repo-local evidence under `.vizier/`.
- Keeps workspaces clean via temporary worktrees and repo-level session logs.
- Operates 100% Git-native — every artifact is a branch, commit, or tracked file you can inspect locally.

## Why You'd Use It
Use Vizier when you need guardrails for AI-assisted development without outsourcing your repo to another SaaS surface.

- You want agents to propose edits, but raw writes into your repo are unsafe.
- Your commits explain what changed but not the ongoing narrative or rationale.
- Compliance teams expect architecture docs, review records, and traceable AI usage.
- You prefer a repo-local control plane that travels with Git history.

## Core Concepts

### Snapshot
A single-page story bible that captures both CODE STATE (observable behaviors/interfaces) and NARRATIVE STATE (themes, tensions, open threads). Treat it like a diffable frame of truth.

- Always read-before-write; prefer minimal, diff-like updates over rewrites.
- Cross-link snapshot paragraphs to the threads and TODOs they inform.
- Ground every edit in evidence from the codebase or user behavior — never invent internals.

### Threads & TODOs
Threads are narrative arcs; TODOs are the Chekhov's guns that resolve each tension.

- Every TODO advances a named thread and cites the relevant slice of the Snapshot.
- Tasks stay product-level by default; reach for implementation detail only when safety or explicit requests demand it.
- No "investigate X" placeholders — each TODO commits to an observable outcome with acceptance criteria.

### Manual commit hold (--no-commit)
Assistant-backed commands normally commit immediately. Pass the global `--no-commit` flag (or set `[workflow] no_commit_default = true` in `.vizier/config.toml`) to leave `.vizier` and code edits staged/dirty instead. This lets you inspect draft/approve/review changes inside their worktrees before writing history. `vizier merge` still requires normal commits; rerun without the flag once you are ready to finalize the merge.

### Agent Control Plane
Vizier is the mediator between agents and Git.

- Agents never write to the repo directly; they operate through Vizier commands that stage and commit behind disposable worktrees.
- `draft/approve/review/merge` map directly onto Git branches, commits, and merge sentinels.
- The Auditor, gates, and stored artifacts keep every change auditable and reversible.

### Outcomes & Session Logging
Every command emits a one-line Outcome plus a structured JSON record. Full transcripts, token usage, and gate facts live under `.vizier/sessions/<id>/session.json` so you can audit what happened long after the CLI exits.

## How It Fits Your Dev Cycle

### Human-Driven Flow
Use Vizier as your narrative maintainer even when no agents are involved.

- `vizier ask "..."` captures a directive, updates the Snapshot/TODOs, and exits with a concise outcome.
- Vizier applies the default-action posture: unless you opt out, conversations update the narrative artifacts automatically.
- `vizier save` stages `.vizier/` edits plus code changes, runs the Auditor, and lands commits without disturbing existing staged work.

```bash
vizier ask "summarize open threads around stdout/stderr"
vim src/... # make optional manual edits
vizier save -m "feat: tighten io contracts"
```

### Agent-Heavy Flow
Codex-backed workflows stay isolated on draft branches so you can review work before it merges.

1. `vizier draft "add retry logic to the API client"` → creates `draft/<slug>` with `.vizier/implementation-plans/<slug>.md` committed on that branch via a disposable worktree.
2. `vizier approve <slug>` → replays the plan on the draft branch, staging commits without touching your checkout.
3. `vizier review <slug>` → runs the configured checks (defaults to `cargo check --all --all-targets` + `cargo test --all --all-targets` when Cargo exists), streams Codex’s critique to the terminal/session log (no `.vizier/reviews` artifacts), updates the plan status, and can apply targeted fixes.
4. `vizier merge <slug>` → performs a non-fast-forward merge into the target branch, embedding the stored plan under an `Implementation Plan:` section of the merge commit and running any configured CI gate. By default the implementation edits are squashed into a single code commit before the merge commit is written; pass `--no-squash` (or set `[merge] squash = false` in `.vizier/config.toml`) to keep the legacy multi-commit history.

Each Vizier action lands a single commit that bundles code edits with canonical narrative assets (`.vizier/.snapshot` and root-level TODO threads). Plan documents, `.vizier/tmp/*`, and session logs stay as scratch artifacts and are filtered out automatically.

Worktrees keep the primary checkout clean throughout. See `docs/workflows/draft-approve-merge.md` for the full choreography, including completions, gating, and troubleshooting tips.

## Quickstart

```bash
# Install
cargo install vizier

# Initialize in a repo
vizier init-snapshot

# Everyday commands
vizier help
vizier ask "add retry logic to the API client"
vizier save -m "feat: capture retry rationale"
vizier draft --name stdout-stderr "refresh stdout/stderr contract"
vizier approve stdout-stderr
vizier review stdout-stderr
vizier merge stdout-stderr
```

## Workflows & Docs
- `Draft → Approve → Review → Merge`: `docs/workflows/draft-approve-merge.md`
- Snapshot/TODO discipline (coming soon) will live under `docs/`
- Protocol mode vs human mode, stdout/stderr contracts, and other threads are tracked in `.vizier/.snapshot`

## Configuration & Agent Backends
Tune Vizier via repo-local files so settings travel with commits.

- `.vizier/config.toml` defines agent scopes (`[agents.ask]`, `[agents.save]`, `[agents.draft]`, `[agents.approve]`, `[agents.review]`, `[agents.merge]`), merge defaults (e.g., `[merge] squash = true` to keep two commits per plan), Codex options, and prompt overrides. CLI flags sit above these scopes.
- `.vizier/*.md` prompt files (BASE_SYSTEM_PROMPT, IMPLEMENTATION_PLAN_PROMPT, REVIEW_PROMPT, MERGE_CONFLICT_PROMPT, etc.) override baked-in templates per command.
- `.vizier/COMMIT_PROMPT.md` (or `[prompts.commit]` in config) replaces the baked Linux kernel-style commit template if your team prefers a different format.
- `example-config.toml` documents every knob, precedence rule (`CLI → scoped agent → default agent → legacy`), and Codex vs wire backend settings.

## Philosophy: Narrative Maintainer
Vizier treats software development as story editing, not just diff management.

- Code is story, not an index; the Snapshot is the single-frame story bible.
- Every TODO is a Chekhov's gun — specific, contextual, and inevitable to resolve.
- Evidence beats speculation; ground changes in observable behavior and tests.
- Evolve existing threads instead of spawning duplicates; continuity beats churn.
- Operators stay in control; agents amplify maintainer intent but never free-drive.
