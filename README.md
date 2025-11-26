# Vizier

**A managed VCS harness for using agents.**

Vizier is the control plane between your agents and your Git repository. It's not an AI assistant — it's the infrastructure that lets you safely use *your* agents (Claude Code, Cursor, custom scripts) without letting them write directly to your repo.

**Bring Your Own Agent.** Vizier doesn't care which agent you use. It cares *how* that agent interacts with version control: isolated branches, audited commits, narrative tracking, and compliance gates.

## How to Use It

**Prerequisites**:
- Rust toolchain to build from source
- The built-in agent scripts use `jq` to parse events, but this can be changed to whatever you like
- Automated testing relies on `git`

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
vizier draft --name rate-limiting "add rate limiting to API client"

# 2. Implement the plan (agent-backed)
vizier approve rate-limiting

# 3. Review and optionally apply fixes
vizier review rate-limiting

# 4. Merge to main with embedded plan and let the agent deal with merge conflicts
vizier merge rate-limiting --auto-resolve-conflicts
```

## What to Expect

For the clearest example, check out this project's commit history.

**After running commands, you'll see something like:**
```
Outcome    : Save complete
Session    : .vizier/sessions/3ab53405-72c1-42c6-a763-1d9bec689ed9/session.json
Code commit: 957ccbf4
Mode       : auto
Narrative  : committed
Agent      : backend agent • runtime codex • scope ask • exit 0 • elapsed 16.73s
Exit code  : 0
Duration   : 16.73s
```

A variety of the built-in prompts encourage the agent to maintain a `.vizier/narrative/snapshot.md` file as an at-a-glance outline of the project state--this is intended for both future agent and human eyes. And of course, the prompts can be configured to change or ignore this entirely.

**Configuration:** See `example-config.toml` and `docs/config-reference.md` for agent backends, workflow settings, and prompt customization.

---

This is mostly just an exploration of how I meaningfully work with agents. Ideas or suggestions for change are wholly welcome.
