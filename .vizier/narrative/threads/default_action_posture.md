# Default-Action Posture (DAP)

Thread: Default-Action Posture (cross: Outcome summaries, stdout/stderr contract + verbosity)

Snapshot anchor
- Active threads — Default-Action Posture (DAP) (Running Snapshot — updated).
- Narrative theme — Reduce operator friction; Story-editor discipline; Commit-style epilogues.

Problem/Tension
- If narrative maintenance is opt-in, the snapshot drifts and operators have to remember “now update the snapshot,” which increases friction and breaks the “snapshot-first” posture.
- If narrative maintenance is always-on without a clear opt-out and update discipline, the snapshot accumulates noise, duplicates, and speculative “investigate X” entries.
- Output can accidentally become verbose (or leak internal deltas), undermining the “commit-style epilogue” contract and making runs harder to audit.

Desired behavior (Product-level)
- Default action: treat every user input as authorization to update `.vizier/narrative/snapshot.md` and (when needed) `.vizier/narrative/threads/*.md`.
- No-wait execution: do not wait for a separate "please update" request when no opt-out signal is present; the turn itself is the authorization surface.
- Explicit opt-out: when users signal “no update” for a turn (e.g., `no-op:` / `discuss-only:` / explicit “do not update”), do not change narrative files.
- Snapshot discipline: merge into existing threads, update minimally, de-duplicate, and ground claims in observable evidence (code, tests, or user reports).
- Glossary lockstep: when snapshot edits introduce or clarify high-signal terms, update `.vizier/narrative/glossary.md` in the same change.
- Output contract: respond with only a short, commit-message-like summary of narrative changes; keep detailed diffs/deltas internal to `.vizier` rather than printing them verbatim.
- Outcome alignment: the standardized Outcome epilogue / outcome.v1 JSON (once implemented) should reflect when DAP acted and which narrative files changed.

Acceptance criteria
1) Default behavior
   - Given a user input that does not explicitly opt out, the snapshot is updated to reflect the new plot point (or to evolve an existing thread) without spawning duplicates.
2) Opt-out behavior
   - Given an explicit “no update” signal, narrative files remain unchanged and the response indicates a no-op.
3) Noise control
   - New snapshot entries avoid “investigate X” tasks; each entry ties a tension to a concrete, observable behavior change or acceptance signal.
4) Output contract
   - User-facing responses are concise (commit-style) and do not include raw snapshot deltas.
5) Glossary lockstep
   - Snapshot updates that add or alter terms update the glossary in the same change set.
6) IO contract integration
   - As Outcome summaries standardize, DAP actions appear in the same stdout/stderr + protocol-mode matrix (no ANSI leakage; deterministic final outcome).

Status
- DAP is currently a prompt-level contract enforced by the base documentation prompt (`vizier-core/src/lib.rs::DOCUMENTATION_PROMPT` / `SYSTEM_PROMPT_BASE`) plus repository guidance; the CLI-side Outcome standardization needed to fully “close the loop” remains tracked under Outcome summaries + stdout/stderr contract.

Pointers
- `vizier-core/src/lib.rs` (documentation prompt + scope rules)
- `.vizier/narrative/snapshot.md` (single canonical snapshot)
- `.vizier/narrative/threads/stdout_stderr_contract_and_verbosity.md`

Update (2026-01-24)
- Added a canonical thread doc for DAP and cross-linked it from the snapshot so the “default action” contract has a single home with acceptance criteria.
Update (2026-01-27)
- Narrative upkeep now expects direct file edits under `.vizier/narrative/` (no Vizier CLI tooling), stays within repo boundaries (no network access unless explicitly authorized), and preserves the explicit opt-out rule.
Update (2026-01-30)
- Clarified glossary lockstep expectations so snapshot edits and term definitions stay paired under the Default-Action Posture.
Update (2026-02-01)
- Stopgap enforcement: narrative edits proceed only when the user explicitly instructs updates, honoring the AGENTS guardrail while the DAP precedence rule remains unresolved. Clarified that the default-action intent treats every user input as authorization unless an explicit opt-out is given.
Update (2026-02-02)
- Clarified that explicit update instructions authorize edits to supporting narrative thread docs (not just snapshot/glossary) while keeping updates minimal and commit-style.
Update (2026-02-03)
- Documented that tasks now explicitly include "Update the snapshot, glossary, and supporting narrative docs as needed" as the guardrail-satisfying authorization; when absent, narrative updates are treated as a no-op while DAP precedence remains unresolved.
Update (2026-02-04)
- Added a response cue: when users signal lost context, surface the relevant snapshot slice and active threads before proceeding.
Update (2026-02-05)
- Reaffirmed the snapshot framing as two slices (Code state and Narrative state) and that DAP updates should preserve the split while keeping user-facing output to a commit-style summary with snapshotDelta retained only inside `.vizier`.
Update (2026-02-05, follow-up)
- Clarified task-envelope execution: when a turn bundles the explicit update instruction with inline snapshot/thread context, narrative upkeep is treated as immediately authorized for the first response, still anchored to on-disk `.vizier/narrative/*` canonical files.
Update (2026-02-06)
- Clarified response wording: narrative-maintenance turns should return a short commit-style summary (not raw delta output), while detailed `snapshotDelta` remains internal to `.vizier`.
Update (2026-02-06, follow-up)
- Clarified guardrail parsing: the explicit update instruction is still valid when wrapped in `<task><instruction>...</instruction></task>`, so formatting does not block authorized first-response narrative upkeep.
Update (2026-02-06, task-envelope follow-up)
- Clarified no-wait execution posture: when the task envelope contains the explicit update instruction, editorial updates are expected in the first response, and only an explicit no-update signal suppresses narrative edits for that turn.
