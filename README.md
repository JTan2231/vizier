# Vizier

**A managed VCS harness for using agents.**

Vizier is the control plane between your agents and your Git repository. It's not an AI assistant — it's the infrastructure that lets you safely use *your* agents (Claude Code, Cursor, custom scripts) without letting them write directly to your repo.

**Bring Your Own Agent.** Vizier doesn't care which agent you use. It cares *how* that agent interacts with version control: isolated branches, audited commits, narrative tracking, and compliance gates.

## How to Use It

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

**Agent workflow** (draft → approve → review → merge):

```bash
# 1. Create a plan on a draft branch
vizier draft "add rate limiting to API client"

# 2. Implement the plan (agent-backed)
vizier approve rate-limiting

# 3. Review and optionally apply fixes
vizier review rate-limiting

# 4. Merge to main with embedded plan
vizier merge rate-limiting
```

## What to Expect

**After running commands, you'll see:**
```
Outcome: Save complete
Files: src/client.rs (M), .vizier/.snapshot (M)
Session: .vizier/sessions/abc123/session.json
```

Vizier maintains `.vizier/.snapshot` (project state overview) and narrative docs under `.vizier/narrative/threads/` as you work. Every change is Git-tracked and auditable. Your working tree stays clean via temporary worktrees. No external services, no lock-in — just Git with opinions about commit hygiene.

**Configuration:** See `example-config.toml` and `docs/config-reference.md` for agent backends, workflow settings, and prompt customization.

---

For questions, issues, or contributions, see `.vizier/.snapshot` for project status and `AGENTS.md` for agent integration notes.
