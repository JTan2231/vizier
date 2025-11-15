---
plan: we-need-to-cull-a-few
branch: draft/we-need-to-cull-a-few
status: draft
created_at: 2025-11-15T23:53:40Z
spec_source: inline
---

## Operator Spec
we need to cull a few commands: snapshot (init-snapshot can stay), clean, and docs all need to go

## Implementation Plan
## Overview
Operators asked to slim down the CLI by retiring the rarely used `vizier snapshot …`, `vizier clean`, and `vizier docs prompt` commands while keeping the single-purpose `vizier init-snapshot` entry point. This aligns with the snapshot’s “lean backlog” and “bureaucratic enforcement” themes by forcing snapshot bootstrapping and TODO hygiene to flow through modern guardrails (`init-snapshot`, DAP, forthcoming TODO GC) instead of side utility commands. The change primarily impacts CLI users and any automation that still shells out to the deprecated subcommands; documentation and the snapshot/TODO threads must reflect the new surface so reviewers see an auditable story.

## Execution Plan
1. **Prune the CLI subcommand surface (`vizier-cli/src/main.rs`)**  
   - Drop the `Docs`, `Snapshot`, and `Clean` variants from the `Commands` enum along with their associated `*Cmd` structs/enums, keeping only the top-level `InitSnapshot` subcommand so `vizier init-snapshot` remains available.  
   - Update the main `match` dispatch to remove the obsolete arms and ensure `run_snapshot_init` is only reachable via the alias, with doc comments adjusted to reflect the canonical syntax.  
   - Acceptance: running `vizier --help` no longer displays `docs`, `snapshot`, or `clean`; `vizier init-snapshot --help` still documents the bootstrap options.

2. **Remove the retired command implementations and dependencies (`vizier-cli/src/actions.rs`, `vizier-core/src/prompting.rs`)**  
   - Delete the `docs_prompt` async helper and any imports it required, and ensure we either remove or intentionally retain the underlying `vizier-core::prompting` module for future architecture-doc gating (per `.vizier/todo_architecture_doc_gate_and_commit_history.md`). If we keep it, add a brief comment referencing that TODO so the unused code is justified.  
   - Remove the `clean` function and its call site, along with any supporting wiring that is now unused (e.g., CLI argument parsing). Verify that shared helpers like `get_editor_message` remain intact for `vizier save`.  
   - Acceptance: `cargo check -p vizier-cli` succeeds, and attempts to run `vizier clean`/`vizier docs …` fail with the standard “unrecognized subcommand” error.

3. **Refresh documentation and narrative artifacts**  
   - Update `README.md` “Core Commands” to remove the “Documentation Prompts” and “TODO Maintenance” sections, revise Snapshot Management to reference only `vizier init-snapshot`, and add a short note pointing architecture-doc writers to future enforced flows (tying back to the architecture-doc gate TODO).  
   - Amend `.vizier/.snapshot` (Code state + Active thread notes) so it no longer claims that `vizier docs prompt` exists or that ask/clean flows trigger commits, and adjust `.vizier/todo_stdout_stderr_contract_and_verbosity` to remove `clean` from the outcome matrix; reference the TODO garbage-collection thread as the replacement for manual cleanup.  
   - Acceptance: README’s first screenful and Core Commands sections show the trimmed surface, and snapshot/TODO files describe the new reality so auditors don’t see stale instructions.

## Risks & Unknowns
- Some operators or scripts may still rely on `vizier clean` or `vizier docs prompt`. We need to mention the removals in README/Core Commands and, if necessary, in a release note so adopters know to shift toward DAP/TODO GC or manual doc scaffolding until the architecture-doc gate lands.  
- Removing the CLI entry point cuts off the only shipped way to scaffold documentation templates today; if the architecture-doc gate slips, we may have a functionality gap. Reconfirm with stakeholders whether keeping `prompting.rs` for internal reuse is acceptable and note how operators can copy templates manually in the interim.  
- Ensure no other modules (tests, integration fixtures) expect the deleted commands; otherwise, additional code surgery might be required.

## Testing & Verification
- `cargo fmt` to keep formatting clean after deleting large blocks.  
- `cargo clippy -p vizier-cli` or at least `cargo check -p vizier-cli` to ensure no dangling references remain.  
- Targeted smoke runs of `vizier --help` and `vizier init-snapshot --help` (via integration tests or manual invocation) to confirm the command list matches documentation.  
- Existing test suite (`cargo test -p vizier-cli`, `cargo test`) to catch regressions in the CLI wiring after the command removals.

## Notes
- Coordinate messaging with the Default-Action Posture + TODO GC threads so there’s a clear replacement story for TODO remediation once `vizier clean` disappears.  
- When the architecture-doc gate work starts, reuse or relocate `vizier-core::prompting` so doc scaffolding re-enters via the gated workflow instead of a standalone command.  
- Narrative delta this run: authored `.vizier/implementation-plans/we-need-to-cull-a-few.md` describing the scope for culling the legacy commands.
