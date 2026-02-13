# Feature Spec: Portable Multi-Page Man Documentation for Vizier

## Summary
Add a portable man-page system for Vizier so operators can read command and workflow docs with `man` after installation, without needing the source tree.

This includes:
- a clear page taxonomy (`man1`, `man5`, `man7`)
- deterministic generation for command reference pages
- installation of all shipped pages via `install.sh`
- release/distribution artifacts that include the pages alongside the binary

## Goals
- Make Vizier documentation available through standard `man` tooling after install.
- Split docs by responsibility so users can find the right depth quickly.
- Keep command references synchronized with CLI flags/subcommands.
- Ensure install/uninstall flow tracks every man page in the manifest.
- Support source-free consumption (binary + docs artifact layout).

## Non-Goals
- Replacing existing Markdown docs under `docs/user/` and `docs/dev/`.
- Documenting hidden internal commands (`__complete`, `__workflow-node`, `build __materialize`, `build __template-node`).
- Changing scheduler behavior, command semantics, or config behavior.
- Introducing a dependency on heavyweight TUI/doc systems.

## Page Set and Sections

### `vizier(1)`
- Purpose, top-level synopsis, global options, command index.
- Short operator examples for common flows.
- References to deeper pages (`vizier-jobs(1)`, `vizier-build(1)`, `vizier-config(5)`, `vizier-workflow(7)`).

### `vizier-jobs(1)`
- Full `vizier jobs` surface (`list`, `schedule`, `show`, `status`, `retry`, `approve`, `reject`, `tail`, `attach`, `cancel`, `gc`).
- Scheduler-facing operator contract:
  - state model and visibility
  - dependency orchestration (`--after`)
  - approval gating and retry/cancel behavior
  - `--follow`/attach/tail semantics
  - schedule format/watch constraints

### `vizier-build(1)`
- `vizier build` and `vizier patch` orchestration.
- Pipeline modes and resume semantics.
- Relationship to approve/review/merge stage execution.

### `vizier-config(5)`
- Config file locations and merge precedence.
- Key sections from `docs/user/config-reference.md`.
- Prompt alias/kind constraints from `docs/user/prompt-config-matrix.md`.
- Relevant environment variables (`VIZIER_CONFIG_FILE`, `VIZIER_CONFIG_DIR`, `VIZIER_AGENT_SHIMS_DIR`, `VIZIER_PAGER`).

### `vizier-workflow(7)`
- Command composition patterns and operator runbooks.
- Draft/approve/review/merge lifecycle and resume/conflict flows.
- Gate behavior at a user-contract level (not implementation internals).

## Source of Truth and Generation Model
- Command pages (`man1`) are generated from Clap command definitions to reduce drift.
- Concept pages (`man5`, `man7`) are maintained as curated authored docs.
- Generated output is checked into `docs/man/` so packagers can consume docs without running code generation at install time.

### Required Layout
- `docs/man/man1/vizier.1`
- `docs/man/man1/vizier-jobs.1`
- `docs/man/man1/vizier-build.1`
- `docs/man/man5/vizier-config.5`
- `docs/man/man7/vizier-workflow.7`

Symlink-only layout is not sufficient once multiple pages exist; each shipped page must be a real file in its section directory.

## Build and Install Contract

### Generation Command
Add a repeatable command (for example `cargo run -p vizier --bin gen-man`) that:
- renders/updates generated `man1` pages
- supports `--check` mode for CI drift detection

### Installer Behavior (`install.sh`)
Update installer/uninstaller to:
- install all man files under `docs/man/man*/` into `$MANDIR/man*/`
- record all installed pages in the manifest
- remove all recorded man pages on uninstall
- print all installed man-page targets in install summary

## Source-Free Distribution Contract
Ship release artifacts that include:
- `bin/vizier`
- `share/vizier/agents/...`
- `share/man/man1/*.1`, `share/man/man5/*.5`, `share/man/man7/*.7`

Result:
- users can install from artifact/package and run `man vizier`, `man vizier-jobs`, etc., with no source checkout.

## Acceptance Criteria
- `docs/man/` contains the full page set above with section-correct filenames.
- Generated command pages are reproducible and CI-verifiable (`--check` fails on drift).
- `install.sh` stages and uninstalls all shipped man pages, with manifest parity.
- Installed docs are discoverable with standard `man` lookup from `$MANDIR`.
- `docs/user/installation.md` documents multi-page install layout and lookup examples.
- Existing single-page references remain backward compatible via `SEE ALSO`.

## Test Plan

### Integration (`tests/src/install.rs`)
- Assert staged install includes all expected man pages (man1/man5/man7).
- Assert manifest contains all installed man-page paths.
- Assert uninstall removes all installed man pages.
- Extend dry-run assertions to include all planned man-page install operations.

### Generation checks
- Add a CI-invoked check that generated man1 output is current.
- Fail fast when command metadata changes without regenerating man docs.

## Documentation Updates (Implementation Phase)
- Update `docs/user/installation.md` with:
  - installed man directory tree
  - examples for `man vizier`, `man vizier-jobs`
  - `MANPATH`/`man -M` fallback examples for non-standard prefixes
- Update `docs/user/build.md` and workflow docs where they reference command help/discoverability.
- Keep `docs/user/config-reference.md` and `docs/user/prompt-config-matrix.md` as canonical deep references linked from `vizier-config(5)`.

## Rollout
1. Land page taxonomy and initial authored `man5`/`man7` content.
2. Add command-page generator and commit generated `man1` pages.
3. Update installer + tests for multi-page install/uninstall.
4. Update user docs.
5. Enable CI drift check for generated pages.
