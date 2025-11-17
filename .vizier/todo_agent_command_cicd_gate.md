# CI/CD gate for agent-backed commands

Thread: Agent workflow orchestration (cross: Outcome summaries, Session logging, stdout/stderr contract, pluggable agent backends)

Tension
- Agent-backed commands can currently report success even when repo checks or external CI fail, so “green” runs don’t reliably mean that quality/safety gates passed.
- `vizier review` already runs configurable checks, but its results are not yet treated as a reusable gate primitive that other agent flows can depend on.

Desired behavior (Product-level)
- Define a CI/CD gate concept that agent-backed commands can require: each such command declares which gate profile it uses (for example, local check commands, a delegated remote pipeline, or both), expressed in repo config rather than hard-coded per command.
- A gate must complete successfully for an agent-backed command to be considered successful; failures surface as blocked outcomes with clear reasons while preserving artifacts for debugging (diffs, reviews, logs, session records).
- Gate configuration stays small and composable: repositories define a handful of gate profiles that multiple commands can share, instead of a sprawling set of per-command flags.
- Outcome epilogues and session logs record which gate was evaluated, its status, and a short check summary so auditors and downstream tools can trust that “success” implies “gate passed.”

Acceptance criteria
- Introduce a gate abstraction that can be bound to agent-backed commands (`vizier ask`, `vizier draft`, `vizier approve`, `vizier review`, `vizier merge`, `vizier save`) without changing their top-level UX; default repo configuration defines at least one sensible gate profile (for example, a checks gate that reuses existing `review.checks` commands).
- When a gate is attached to a command:
  - Passing the gate marks the command outcome as successful; human epilogues and outcome.v1 JSON include `gate: {name, status: "passed"}` alongside existing A/M/D/R facts.
  - Failing the gate marks the operation as blocked; agent output and artifacts remain available, exit codes reflect failure, and Outcome explains which gate failed and where to inspect check results.
- When no gate is configured for a command, behavior matches today but Outcome clearly indicates `gate: {status: "none"}` so scripts and operators do not assume checks ran.
- Gate execution honors stdout/stderr and mode-split contracts: no ANSI in non-TTY contexts; structured gate results are available via outcome.v1 JSON and align with human epilogues; gate logs remain distinguishable from agent-progress history.
- Integration tests cover at least: a passing gate, a failing gate that blocks success while preserving artifacts, and a command with no gate configured; tests assert consistent Outcome/session recording and exit codes across TTY vs non-TTY and `--json`/protocol-style modes.

Status
- Merge-time CI/CD gate shipped for `vizier merge` via `[merge.cicd_gate]` plus per-run overrides; README, workflow docs, and integration tests (`test_merge_cicd_gate_executes_script`, `test_merge_cicd_gate_failure_blocks_merge`, `test_merge_cicd_gate_auto_fix_applies_changes`) are in place.
- Remaining work focuses on generalizing the gate abstraction beyond merge (ask/save/draft/approve/review), emitting structured gate facts into Outcome/session logs, and allowing repositories to define reusable named gate profiles instead of merge-only wiring.

Pointers
- Agent workflow orchestration thread in `.vizier/.snapshot` (Active threads: Agent workflow orchestration).
- `vizier review` checks and `[review.checks]` configuration as the initial gate surface.
- Outcome summaries and stdout/stderr contract TODOs for reporting and IO rules.
- Session logging JSON store for recording per-command gate decisions.

Update (2025-11-17): `vizier merge` now treats `[merge.cicd_gate]` as the gate definition. The CLI resolves repo config + per-run overrides (`--cicd-script`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, `--cicd-retries`), executes the script after staging the merge commit (including `--complete-conflict` resumes), surfaces the captured stdout/stderr on failure, and optionally lets Codex attempt remediation when the merge backend is Codex. README and `docs/workflows/draft-approve-merge.md` describe the new behavior, and integration tests (`test_merge_cicd_gate_executes_script`, `test_merge_cicd_gate_failure_blocks_merge`, `test_merge_cicd_gate_auto_fix_applies_changes`) cover pass/fail/auto-fix scenarios. Remaining work: generalize the gate abstraction beyond merge (ask/save/draft/approve/review), emit structured gate facts via outcome.v1/session logs, and allow repositories to define reusable named gate profiles rather than merge-only wiring.
