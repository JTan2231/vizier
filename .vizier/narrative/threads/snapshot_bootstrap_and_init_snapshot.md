# Repository initialization contract and bootstrap (`vizier init`)

Thread: Repository initialization contract (`vizier init`) — cross: Narrative storage, Snapshot-first prompting, Configuration posture + defaults

## Tension
- Bootstrap behavior was implicit and scattered across command setup paths, which made it hard for operators and CI to answer a deterministic question: "is this repo initialized for Vizier?"
- Earlier bootstrap guidance centered on `vizier init-snapshot`, which mixed narrative framing concerns with initialization mechanics and left room for confusion about required durable files vs ephemeral runtime directories.
- Pre-dispatch setup mutated `.vizier` in some paths, so a validation-style check could still create directories, undermining read-only expectations.

## Desired behavior (product-level)
- `vizier init` is the canonical bootstrap command.
- Initialization is contract-driven and machine-checkable:
  - Durable markers: `.vizier/narrative/snapshot.md` and `.vizier/narrative/glossary.md`.
  - Required `.gitignore` runtime coverage: `.vizier/tmp/`, `.vizier/tmp-worktrees/`, `.vizier/jobs/`, `.vizier/sessions/`.
- Mutating init is idempotent and safe to rerun:
  - Creates missing durable markers with starter content.
  - Appends missing ignore rules without reordering unrelated `.gitignore` content or duplicating equivalent patterns.
  - Never overwrites existing marker file contents by default.
- `vizier init --check` validates the same contract without mutating files and exits non-zero with an explicit missing-item list when requirements are not met.

## Acceptance criteria
- `vizier init` on an uninitialized repo creates durable marker files and required ignore entries, then reports initialization applied.
- Re-running `vizier init` on a satisfied repo produces no file-content changes and reports already satisfied.
- `vizier init --check` exits 0 only when durable markers and required ignore coverage are present.
- `vizier init --check` exits non-zero with explicit missing markers/ignore entries when requirements are absent.
- Check mode is non-mutating: it does not create `.vizier`, `.vizier/jobs`, or `.vizier/sessions` as a side effect.
- Outside a Git repository, `vizier init` fails with an explicit non-git error.

## Status
- Shipped v1:
  - New `vizier init` command surface with `--check`.
  - Shared init-state evaluator used by mutate and check paths.
  - Idempotent durable scaffolding and equivalence-aware `.gitignore` reconciliation.
  - Dispatch now bypasses pre-command `.vizier` directory creation for `vizier init` so `vizier init --check` remains read-only.
  - Integration coverage added for fresh/partial/full/check/outside-git/permission-failure paths.
- Follow-up:
  - README/AGENTS are human-authored; align remaining bootstrap wording there via human-authored updates when desired.

## Pointers
- CLI wiring: `vizier-cli/src/cli/args.rs`, `vizier-cli/src/cli/dispatch.rs`
- Init action + evaluator: `vizier-cli/src/actions/init.rs`
- Integration tests: `tests/src/init.rs`
- Command reference + docs: `docs/man/vizier.1`, `docs/user/installation.md`, `docs/user/workflows/draft-approve-merge.md`
