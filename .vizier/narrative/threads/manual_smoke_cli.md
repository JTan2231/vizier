# Manual smoke coverage for core CLI

Status (2026-02-14): REFRAMED. Smoke scope now tracks retained commands only (`init`, `list`, `cd`, `clean`, `jobs`, `completions`, `release`) plus removed-command negative checks.

Thread: Manual smoke coverage (cross: Stdout/stderr contract + verbosity, Outcome summaries, Agent workflow orchestration)

Snapshot anchor
- Next moves — Manual smoke coverage (Running Snapshot — updated).

Tension
- Automated tests alone can miss regressions in CLI output contracts (quiet/no-ansi), session logging, and gate flows, so releases can look green while user-visible behavior drifts.

Desired behavior (Product-level)
- Provide a lightweight human smoke checklist covering `vizier init/list/cd/clean/jobs/release/completions` in default, quiet, and no-ansi modes, plus negative checks that removed commands fail as unknown subcommands.
- Smoke runs capture the emitted Outcome lines and the `.vizier/sessions/<id>/session.json` paths so auditors can trace what was exercised.
- Any regression found during a smoke run is recorded as a TODO tied to the relevant snapshot thread instead of disappearing into chat.
- The checklist lives alongside the existing workflow docs so operators know when/how to run it without learning a new surface.

Acceptance criteria
- A documented smoke playbook exists (linked from `docs/user/workflows/draft-approve-merge.md` or a short companion doc) that lists the commands/modes above and the expected observable outcomes (no ANSI in non-TTY/`--no-ansi`, quiet suppresses progress, Outcome lines present).
- Running the playbook produces a simple artifact per run (checklist/log) that notes pass/fail per step and the session log references; runs can occur on disposable branches/worktrees without polluting the primary checkout.
- Release notes or gate outcomes reference the most recent smoke run or explicitly state it was skipped; regressions from smoke runs create TODO entries aligned to snapshot threads.
- Quiet/no-ansi variants in the playbook assert that progress/history lines respect verbosity/ANSI contracts and that the final Outcome still appears on stdout.

Pointers
- vizier-cli (retained command flows and removed-command negative coverage)
- docs/user/workflows/draft-approve-merge.md
- .vizier/sessions/
