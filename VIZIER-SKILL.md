# Vizier Operator Skill - Feature Spec

Goal: define a Codex skill ("vizier-operator") that runs Vizier effectively, with first-class guidance for configuration setup.

This spec applies the ontology in ONTOLOGY.md to a concrete skill package and defines the artifacts, behavior, and acceptance criteria for the skill.

---

## 1) Problem Statement

We need a single skill that turns Codex into a reliable Vizier operator:
- Understands Vizier's ontology (snapshot, plans, gates, worktrees, outcomes, session logs).
- Runs the correct Vizier commands for common workflows.
- Sets up and verifies configuration for agent selection, prompts, and gates.
- Respects project guardrails (do not edit README.md or AGENTS.md unless a human requests it).

---

## 2) Scope

In scope:
- Operating the Vizier CLI end-to-end for ask/save and draft/approve/review/merge workflows.
- Configuration setup guidance and validation (`vizier plan --json`, config layering, agent selectors, prompt profiles, gates).
- Narrative and snapshot discipline (read first, update only when the workflow calls for it).
- Use of worktrees and plan inventory (`vizier list`, `vizier cd`, `vizier clean`).
- Recovery behavior for failed approvals, reviews, and merges.

Out of scope:
- Editing README.md or AGENTS.md (human-only).
- Creating or modifying Vizier source code (this is a usage skill, not a maintainer skill).
- Non-Vizier Git workflows or custom orchestration beyond the CLI.

---

## 3) Ontology Mapping (ONTOLOGY.md)

### Intent Layer
Skill:
- id: vizier-operator
- name: Vizier Operator
- description: Run Vizier safely and effectively, including configuration setup.
- scope: Vizier CLI workflows, narrative management, configuration setup, audit trail verification.
- triggers:
  - Keywords: "vizier", "draft", "approve", "review", "merge", "snapshot", ".vizier"
  - Context: repo contains `.vizier/` or user asks about Vizier workflows

### Capability Layer
Capability: orient
- verbs: read, inspect, summarize
- targets: snapshot, narrative docs, config, prompts, plan inventory
- outcomes: current posture understood before action

Capability: run-workflow
- verbs: draft, refine, approve, review, merge, ask, save, list, cd, clean
- targets: plan, draft branch, worktree
- outcomes: auditable changes and expected CLI outputs

Capability: configure
- verbs: create, edit, verify
- targets: global config, repo config, prompt profiles, agent selectors, gates
- outcomes: resolved configuration validated via `vizier plan --json`

Capability: audit
- verbs: locate, explain, confirm
- targets: session log, outcome line, gate status
- outcomes: evidence trail for runs

### Prescription Layer (Rules)
Hard constraints (MUST / MUST_NOT):
- MUST read `.vizier/narrative/snapshot.md` before planning or recommending workflow steps.
- MUST respect human-only docs: do not edit `README.md` or `AGENTS.md` unless explicitly asked.
- MUST use Vizier CLI commands rather than re-implementing workflow with manual git steps.
- MUST verify resolved configuration via `vizier plan --json` before a multi-step workflow if config changes are involved.

Soft constraints (SHOULD / SHOULD_NOT):
- SHOULD use the draft -> approve -> review -> merge workflow for non-trivial code changes.
- SHOULD keep guidance grounded in docs/config-reference.md and docs/prompt-config-matrix.md.
- SHOULD point to session logs when explaining outcomes or failures.
- SHOULD prefer repo-local `.vizier/config.toml` for project overrides and global config for defaults.
- SHOULD_NOT attempt background mode (currently disabled in config reference).

### Procedure Layer (Processes)
Process: orient
- inputs: repo path
- outputs: posture summary
- steps: read snapshot, scan docs, identify active draft plans, check config via `vizier plan --json`
- dependencies: vizier CLI
- failure modes: acting without snapshot/context or outdated config

Process: narrative maintenance
- inputs: user intent, repo state
- outputs: updated snapshot/narrative
- steps: choose ask/save, run command, verify outcome and session log
- dependencies: vizier CLI
- failure modes: narrative drift or non-auditable edits

Process: plan workflow
- inputs: operator spec, repo state
- outputs: plan, implemented changes, review critique, merged result
- steps: draft -> refine (optional) -> approve -> review -> merge
- dependencies: vizier CLI, git
- failure modes: missing agent, gate failure, merge conflict

Process: config setup
- inputs: desired behavior (agents, prompts, gates)
- outputs: `.vizier/config.toml` and/or global config
- steps: edit config, run `vizier plan --json`, verify per-scope settings, run target workflow
- dependencies: config reference, prompt-config matrix
- failure modes: misconfigured agent selector or prompt profile, failing gate script

### Evidence Layer
Patterns:
- snapshot-first: read and respect `.vizier/narrative/snapshot.md` before action.
- plan-first: plan changes in `draft/` before implementation.
Anti-patterns:
- bypassing Vizier CLI with manual git commands.
- editing README.md / AGENTS.md without explicit human direction.

Examples (references):
- docs/workflows/draft-approve-merge.md
- docs/prompt-config-matrix.md
- docs/config-reference.md
- example-config.toml

---

## 4) Skill Package Design

Skill name: `vizier-operator`

Directory layout (target):
```
vizier-operator/
  SKILL.md
  references/
    vizier-orientation.md
    vizier-config-setup.md
    vizier-workflows.md
```

Rationale:
- Keep SKILL.md concise with triggers, workflow chooser, and guardrails.
- Put configuration setup details in a dedicated reference file to avoid bloat.
- Include workflow details that mirror docs/workflows/draft-approve-merge.md.

No scripts needed initially. (If repeated config checks become common, add a small helper script later.)

---

## 5) SKILL.md Requirements

SKILL.md must include:
- Trigger description and when to use the skill.
- Mandatory orientation steps:
  - Read `.vizier/narrative/snapshot.md`
  - Read README.md for workflow expectations
  - Read `docs/prompt-config-matrix.md` and `docs/config-reference.md`
  - Scan prompts in `vizier-core/src/lib.rs`
- Workflow chooser:
  - Narrative-only: `vizier ask` or edit + `vizier save`
  - Non-trivial changes: `vizier draft -> approve -> review -> merge`
  - Plan inventory: `vizier list`
  - Worktree access: `vizier cd` / cleanup `vizier clean`
- Guardrails:
  - Do not edit README.md or AGENTS.md unless explicitly asked.
  - Prefer Vizier CLI commands over manual git steps.
  - Validate config via `vizier plan --json` when config changes are involved.

---

## 6) Configuration Setup Guidance (Required Capability)

Skill must explicitly instruct how to configure Vizier, using the canonical docs:

### Configuration sources and precedence
- Without `--config-file`, Vizier loads global config then overlays repo config (`.vizier/config.toml`).
- `vizier plan --json` prints resolved configuration per scope, prompt source, and agent runtime.
- CLI overrides (`--agent`, `--agent-command`, `--agent-label`) apply to the current run only.

### Required setup content (in references/vizier-config-setup.md)
- How to select agent per scope:
  - `[agents.default] agent = "codex" | "gemini" | "<custom>"`
  - `[agents.<scope>] agent = "..."` overrides
- How to override prompt text and per-prompt agents:
  - `[agents.<scope>.prompts.<kind>] { text | path }`
  - Prompt kinds: documentation, commit, implementation_plan, plan_refine, review, merge_conflict
  - Scope x kind usage map references docs/prompt-config-matrix.md
- Documentation context toggles:
  - `[agents.<scope>.documentation] enabled/include_snapshot/include_narrative_docs`
- Gates and checks:
  - `[merge.cicd_gate]` script/retries/auto_resolve
  - `[review.checks] commands = [...]`
  - `[approve.stop_condition]` script/retries
- Merge behavior:
  - `[merge] squash`, `squash_mainline`
  - `[merge.conflicts] auto_resolve`
- Workflow posture:
  - `[workflow] no_commit_default` (paired with `--no-commit`)

### Minimal config example (must match docs)
Provide a minimal example that:
- sets a default agent
- pins a different agent for review or merge if desired
- sets a CI/CD gate script
- adds review checks
- overrides a prompt template via path

The example must explicitly recommend running `vizier plan --json` after changes.

---

## 7) Runtime Behavior (How the skill should operate)

When triggered:
1) Orient: read snapshot, docs, and prompt matrix; inspect config and plan inventory.
2) If config is missing or unclear: guide configuration setup before running workflows.
3) Choose workflow:
   - ask/save for narrative-only updates
   - draft/approve/review/merge for implementation
4) Run the chosen Vizier CLI commands directly.
5) Report outcomes with session log references when available.

Failure handling:
- On agent failure, report the shim error and the failing scope.
- On merge conflicts, follow `vizier merge --complete-conflict` guidance and avoid manual git merges.
- On gate failure, surface the gate script output and propose rerun with auto-fix if configured.

---

## 8) Deliverables

Required:
- `VIZIER-SKILL.md` (this file)
- A concrete skill package created with `init_skill.py` (not in this spec but required to ship)

Expected skill package contents:
- `vizier-operator/SKILL.md`
- `vizier-operator/references/vizier-orientation.md`
- `vizier-operator/references/vizier-config-setup.md`
- `vizier-operator/references/vizier-workflows.md`

---

## 9) Acceptance Criteria

The skill is accepted when:
- It consistently reads snapshot and docs before proposing actions.
- It can guide configuration setup and validate via `vizier plan --json`.
- It selects the correct workflow and runs Vizier commands rather than manual git steps.
- It respects the human-only docs guardrail.
- It can explain outcomes and point to session logs.

---

## 10) Open Questions

- Should the skill include a minimal helper script that runs `vizier plan --json` and prints a short summary?
- Do we want to include a default config template under references, or should we point only to `example-config.toml`?

