# Portable multi-page man documentation

Thread: Portable multi-page man docs (cross: Configuration posture + defaults, Stdout/stderr contract + verbosity)

Snapshot anchor
- Narrative theme — Portable docs posture (Running Snapshot — updated).
- Active threads — Portable multi-page man docs (Running Snapshot — updated).

Tension
- The repo previously shipped a single man page (`docs/man/vizier.1`) with a symlinked `man1/vizier.1`, so installed users could not discover deeper command/workflow/config docs through standard `man` sections.
- Command-reference pages drifted from CLI surfaces because there was no deterministic generation/check workflow tied to CI.
- Installer manifest logic only tracked one man page path, so staged installs/uninstalls could silently miss new documentation pages.

Desired behavior (Product-level)
- Ship a sectioned, portable man layout with real files:
  - `docs/man/man1/vizier.1`
  - `docs/man/man1/vizier-jobs.1`
  - `docs/man/man1/vizier-build.1`
  - `docs/man/man5/vizier-config.5`
  - `docs/man/man7/vizier-workflow.7`
- Generate command pages from live Clap metadata via a repeatable command (`cargo run -p vizier --bin gen-man --`) and enforce drift detection via `--check`.
- Ensure install/uninstall parity: `install.sh` installs every `docs/man/man*/` page into `$MANDIR/man*/`, records each path in the manifest, and removes every recorded page on uninstall.
- Document source-free lookup behavior (`man`, `man -M`, `MANPATH`) in user docs.

Acceptance criteria
- Required sectioned files exist as regular files (not symlinks), and legacy `docs/man/vizier.1` is removed.
- `gen-man --check` exits non-zero on drift and zero when generated pages are current.
- `./cicd.sh` includes `gen-man --check` so stale command pages fail the gate.
- Install tests assert all man targets are staged, manifest-listed, and removed on uninstall, including dry-run visibility.
- Help/man tests assert sectioned layout plus hidden-command exclusions in generated pages.

Status
- Update (2026-02-13): Shipped v1.
  - Added `vizier-cli/src/bin/gen-man.rs` and `vizier-cli/src/man.rs` to generate deterministic `man1` pages from Clap help metadata (`vizier`, `vizier-jobs`, `vizier-build`), including `--check` drift mode.
  - Added authored `man5`/`man7` pages (`vizier-config.5`, `vizier-workflow.7`) and removed legacy `docs/man/vizier.1`.
  - Updated `install.sh` to install all sectioned man pages from `docs/man/man*/`, record each path, and print installed targets.
  - Extended tests (`tests/src/help.rs`, `tests/src/install.rs`, `tests/src/cicd.rs`) and wired `gen-man --check` into `./cicd.sh`.
  - Updated user docs (`docs/user/installation.md`, `docs/user/build.md`, `docs/user/workflows/draft-approve-merge.md`) with multi-page man lookup and generation guidance.
- Update (2026-02-14): Refreshed authored man cross-references for the decomposed workflow docs and root alias posture: `vizier-workflow(7)` now points to the workflow landing page plus focused subpages, and `vizier-config(5)` now lists root compatibility aliases (`docs/config-reference.md`, `docs/prompt-config-matrix.md`) alongside canonical `docs/user/*` references.

Pointers
- Generator: `vizier-cli/src/man.rs`, `vizier-cli/src/bin/gen-man.rs`
- Installer: `install.sh`
- Test coverage: `tests/src/help.rs`, `tests/src/install.rs`, `tests/src/cicd.rs`
- User docs: `docs/user/installation.md`, `docs/user/build.md`, `docs/user/workflows/draft-approve-merge.md`
