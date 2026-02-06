# CI/CD gate for agent-backed commands

Thread: Agent workflow orchestration (cross: Outcome summaries, Session logging, stdout/stderr contract, pluggable agent backends)

Tension
- Agent-backed commands can currently report success even when repo checks or external CI fail, so “green” runs don’t reliably mean that quality/safety gates passed.
- `vizier review` now runs the merge CI/CD gate ahead of critique (auto-remediation disabled) and streams its status into the critique prompt, but gate usage remains merge/review-only and still lacks structured Outcome/session metadata or reuse across ask/save/draft/approve.

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
- Review now executes the merge CI/CD gate before critique (auto-resolve forced off, gate result logged to the Auditor and fed into the critique prompt/commit message) so reviewers see gate status alongside check results; failures warn but do not auto-fix or block the critique flow.
- Approve now supports a stop-condition gate: `[approve.stop_condition]` plus CLI overrides re-run the agent until a repo-local script passes (default three extra attempts), audit every script attempt (status/exit/stdout/stderr), and defer pushes until the passing run; epilogues expose script labels and attempt counts while Outcome/JSON alignment is still pending.
- Remaining work focuses on unifying these per-command gates under reusable profiles (ask/save/draft/approve/review), surfacing structured gate facts in Outcome/session logs, and decoupling gate definitions from merge-specific wiring.

Pointers
- Agent workflow orchestration thread in `.vizier/narrative/snapshot.md` (Active threads: Agent workflow orchestration).
- `vizier review` checks and `[review.checks]` configuration as the initial gate surface.
- Outcome summaries and stdout/stderr contract TODOs for reporting and IO rules.
- Session logging JSON store for recording per-command gate decisions.

Update (2025-11-17): `vizier merge` now treats `[merge.cicd_gate]` as the gate definition. The CLI resolves repo config + per-run overrides (`--cicd-script`, `--auto-cicd-fix`, `--no-auto-cicd-fix`, `--cicd-retries`), executes the script against the on-target worktree (in default squash mode it runs while the squashed implementation commit is staged but before the merge commit is written; in `--no-squash` legacy mode it runs immediately after the merge commit, including `--complete-conflict` resumes), surfaces the captured stdout/stderr on failure, and optionally lets the agent backend attempt remediation when `[agents.merge]` resolves to that backend. README and `docs/workflows/draft-approve-merge.md` describe the new behavior, and integration tests (`test_merge_cicd_gate_executes_script`, `test_merge_cicd_gate_failure_blocks_merge`, `test_merge_cicd_gate_auto_fix_applies_changes`) cover pass/fail/auto-fix scenarios. Remaining work: generalize the gate abstraction beyond merge (ask/save/draft/approve/review), emit structured gate facts via outcome.v1/session logs, and allow repositories to define reusable named gate profiles rather than merge-only wiring.
Update (2025-11-27): `vizier review` now runs the merge CI/CD gate once per review before prompting (auto-resolve disabled even when configured), records the gate result via the Auditor, and threads gate context into the critique prompt/commit author note; failures continue the critique with the failure context so fixes can be applied manually. Remaining work: extend gates to ask/save/draft/approve, and surface gate status in standardized Outcome/session JSON with reusable gate profiles.
Update (2026-02-06): Hardened the repo gate script itself so `./cicd.sh` defaults `CARGO_TARGET_DIR` to `.vizier/tmp/cargo-target` (unless explicitly provided) before running cargo checks. This avoids local permission-denied failures from inherited/root-owned `target/` directories without bypassing the gate, and adds focused script coverage in `tests/src/cicd.rs`.
