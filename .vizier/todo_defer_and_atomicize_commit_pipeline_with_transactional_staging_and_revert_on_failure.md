Title: Defer and atomicize commit pipeline with transactional staging and revert-on-failure

Context
- Current flow triggers commit-related actions inline, increasing the blast radius when any substep fails (index updates, tree writes, hooks, etc.). We need a deterministic, transactional pipeline: stage all commit mutations, perform dry validations, then execute once at the end; if any step fails, perform a full revert.
- This ties into: `cli/src/main.rs` (commit command entry), `cli/src/auditor.rs` (pre-commit checks), `vcs/src/lib.rs` (version control operations), and the planned migration to libgit2.

Acceptance Criteria
- Provide a CommitTransaction API that accumulates all commit-intent operations and material changes but defers writes until `commit()`.
- On `commit()`, execute in a defined order; on any error, perform revert that restores index, working tree, and HEAD to pre-transaction state.
- All public commit paths in CLI switch to this API; no direct writes to VCS occur before the final `commit()`.
- Durable error surface: final result reports which phase failed and includes a revert report (what was undone).

Design
1) CommitTransaction struct (vcs/src/lib.rs)
- Fields:
  - base_head: Oid or String commit id captured at tx start
  - planned_index_changes: Vec<IndexEdit>
  - planned_tree_writes: Vec<TreeWrite>
  - message: String
  - author: Signature
  - committer: Signature
  - hooks_to_run: Vec<Hook>
  - validations: Vec<Box<dyn Validation>>
  - side_effects: Vec<Box<dyn SideEffect>>  // e.g., tags, notes
  - dry_run: bool
  - snapshots: PreCommitSnapshot  // index, worktree status, HEAD

- Methods:
  - begin(repo: &Repository) -> Self: captures HEAD oid and snapshot of index/worktree status
  - stage_index_edit(edit: IndexEdit)
  - write_tree(write: TreeWrite)
  - add_validation(v: impl Validation)
  - add_side_effect(s: impl SideEffect)
  - set_message(msg)
  - set_identities(author, committer)
  - plan_hook(hook)
  - dry_run(bool)
  - commit(self) -> Result<Oid, CommitError>
  - revert(self, reason: CommitError) -> RevertReport  // idempotent, used internally on failure

- Execution order inside commit():
  1. Run validations (pure, read-only)
  2. Apply index edits to an in-memory index (libgit2 Index in memory)
  3. Materialize tree writes into a temporary tree (libgit2 write-tree against in-memory index)
  4. Run hooks with a temp index/tree (no writes to repo refs)
  5. Create commit object pointing to base_head (fast-forward if unchanged)
  6. Apply side_effects (tags/notes) as a post-commit phase
  7. Move refs/HEAD

- On any error between 2-7: discard temp index/tree and restore:
  - Reset index to snapshot
  - Soft reset worktree to snapshot
  - Reset HEAD to base_head
  - Emit RevertReport { failed_phase, restored_head, restored_index_checksum, restored_paths }

Implementation Notes
- Prefer libgit2 (git2 crate): in-memory Index via `Repository::index()` cloned to tmp; `Index::add`/`remove`; `Index::write_tree_to(Repository)` to build tree without writing refs.
- PreCommitSnapshot: save HEAD Oid, index checksum (`Index::checksum()`), and a list of path->stat for modified files (for revert verification only).
- Hook execution: expose trait Hook { fn run(&self, ctx: &HookContext) -> Result<()> } and run with temp tree hash injected into env. Guarantee no side effects on the real repo until ref move.
- Ensure idempotent revert(): only acts if any temp state escaped and double-invocation is safe.

CLI integration (cli/src/main.rs)
- Replace direct commit flow with:
  - let mut tx = CommitTransaction::begin(&repo);
  - populate tx from user flags and detected changes (auditor builds validations)
  - let result = tx.commit();
  - Print success with new Oid or detailed revert report on failure.

Auditor (cli/src/auditor.rs)
- Move pre-commit checks into `Validation` implementations; add them to the transaction rather than executing writes.

Error Model
- CommitError { phase: Phase, source: anyhow::Error, partial: Option<PartialState> }
- Phases: Validate, IndexBuild, TreeWrite, Hook, ObjectCreate, RefMove, SideEffect
- RevertReport contains: phase, reason_summary, actions_taken, verification (checksums match snapshot)

Tests
- Unit: transaction happy path builds commit without mutating repo until ref move
- Unit: each phase injected failure triggers revert and leaves repo identical to snapshot
- Integration: simulate conflicts, hook failure, disk full -> revert
- Property: HEAD and index unchanged after any failing commit attempt

Migration Path
- Implement CommitTransaction behind a feature gate `atomic-commit` default on
- Deprecate older direct commit calls; add compile-time deprecation warnings

Dev Notes
- This aligns with `todo_migrate_git_cli_to_libgit2_in_cli_main_commit_and_diff_paths.md` and `todo_enhance_version_control_composability.md`; update those to reference this transaction primitive and avoid duplicating responsibilities.
