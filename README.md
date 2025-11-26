# Vizier

**A managed VCS harness for using agents.** *(Experimental — expect breaking changes)*

Vizier is the control plane between your agents and your code. The intention here is that we have composable, reversible, and containable means of changing our code while enforcing meaningful development practices.

Because development practices can vary so widely, the intention here is that every possible step can be configured. To name a few surfaces:
- Git interactions
- What agents are run, when, and how
- How agent outputs are displayed
- CICD gating
- Prompts

Hopefully, things are simple enough that both you and your agent will quickly get a handle on personalization.

## How to Use It

**Prerequisites**:
- Rust toolchain to build from source
- The built-in agent scripts use `jq` to parse events, but this can be changed to whatever you like
- Automated testing relies on `git`
- An agent executable--we default to `codex`

**Narrative maintenance**:

```bash
# Install from source
cargo install vizier

# Update the narrative
vizier ask "we're missing retry logic in the API client"

# Have the Vizier maintain the narrative from your code changes
vim src/client.rs
vizier save -m "feat: add retry with exponential backoff"
```

**Agent workflow** (draft → approve → review → merge):

```bash
# 1. Create a plan on a new branch, `draft/rate-limiting` with just an implementation plan
vizier draft --name rate-limiting "add rate limiting to API client"

# 2. Have your agent implement the drafted plan on a separate git worktree
vizier approve rate-limiting

# 3. Have your agent review and optionally apply fixes
vizier review rate-limiting

# 4. Merge the branch to main and let the agent deal with merge conflicts
vizier merge rate-limiting --auto-resolve-conflicts
```

## What to Expect

For the clearest example, check out this project's commit history.

While the agent is working (e.g., `draft`, `approve`, `review`, `merge`), a worktree will be active under `.vizier/tmp-worktrees` in which the agent will be performing its changes on its respective branch.
Whenever the agent finishes its session, it will commit (unless you have `--no-commit` active) to its current branch.

The intention here is to keep the agent's workspace separated, but accessible and auditable.

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

Basically all of Vizier's operations, by default, append to the git history. Check the `git log` often to see the changes Vizier makes.

A variety of the built-in prompts encourage the agent to maintain a `.vizier/narrative/snapshot.md` file as an at-a-glance outline of the project state--this is intended for both future agent and human eyes. And of course, the prompts can be configured to change or ignore this entirely.

**Configuration:** See `example-config.toml` and `docs/config-reference.md` for agent backends, workflow settings, and prompt customization.

---

This is mostly just an exploration of how I meaningfully work with agents. Ideas or suggestions for change are wholly welcome.
