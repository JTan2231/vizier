# Feature: Bootstrap Snapshot and Threads from an Existing Repository with History

Goal: Provide a one-shot command that inspects an existing repo (with deep history) and produces an initial Vizier SNAPSHOT plus seed threads/TODOs, so Vizier becomes useful immediately on established projects.

Why now: Users want to onboard Vizier onto mature repos. Manual curation is costly; an automated baseline accelerates adoption while keeping human editability.

Related thread: “Issue tracking bridge” (Snapshot ‘Project trajectory snapshot (pared)’ → Next moves #7) and general “Narratives/Snapshots” ethos. This command operationalizes that ethos for cold-start.

Behavior (Product level)
- New CLI subcommand: `vizier snapshot init` (alias: `vizier init-snapshot`).
- Runs an expensive, read-only analysis over the current Git repo and working tree to produce:
  1) A SNAPSHOT file (written to .vizier/.snapshot) summarizing CODE STATE surfaces (behaviors/interfaces) and NARRATIVE STATE (themes/tensions) inferred from commits, directories, and docs.
  2) Seed threads as TODO files capturing active tensions/opportunities, cross-linked to the snapshot sections they depend on.
  3) A short report printed to stdout with: files written, high-level findings, and suggested next actions.
- Safety: Never commits. Respects confirm_destructive and non_interactive; prompts before overwriting an existing snapshot unless `--force`.
- Performance: Warns this may take several minutes on large repos; prints progress stages and an estimate.
- Determinism: Same inputs produce stable outputs; includes a generated-at timestamp and commit SHA used for analysis in the snapshot header.
- Idempotence: Re-running updates the snapshot minimally (diff-like) where possible, appending a delta section instead of rewriting when unchanged.

Inputs/Options
- `--force`: overwrite existing .vizier/.snapshot and conflicting TODOs without prompt.
- `--depth <n>`: limit Git history scan (default: heuristic based on repo size; show what was used).
- `--paths <glob...>`: focus analysis on specified subtrees.
- `--exclude <glob...>`: skip paths (e.g., vendor, large binaries).
- `--issues <provider>`: optionally pull open issues (e.g., GitHub) to enrich tensions (reads only). Requires repo remote to be configured; respects tokens in env.

Observable outputs (acceptance)
1) Running `vizier snapshot init` on a non-Vizier repo creates `.vizier/.snapshot` and at least one `todo_*.md` reflecting inferred tensions; stdout shows a summary with commit SHA and elapsed time.
2) On a repo that already has a snapshot, the command refuses to overwrite without `--force` or interactive confirmation and shows what would change.
3) Outputs contain cross-links: TODOs reference snapshot sections; snapshot notes which TODOs were spawned.
4) The snapshot’s CODE STATE focuses on user-visible behaviors (e.g., CLI commands discovered, API endpoints, binaries) with citations (files/paths), not file-by-file listings.
5) The NARRATIVE STATE includes themes and open threads grounded in evidence (commit messages, README, tests). No speculative internals.
6) With `--paths` or `--exclude`, the summary explicitly lists in-scope and out-of-scope areas; snapshot notes this scope.
7) Re-running with no repo changes produces no changes except the generated-at timestamp unless `--force`.

Pointers
- CLI surface: vizier-cli/src/main.rs (subcommand wiring) and README.
- Core analyzers: vizier-core/src/{vcs.rs,walker.rs,observer.rs,display.rs,config.rs}.
- Snapshot writer: vizier-core/src/tools.rs (file IO helpers) and a new snapshot builder module.

Implementation Notes (allowed: safety/correctness + constraints)
- Must be read-only to the Git index and working tree; no staging/unstaging or writes outside .vizier/.
- Include the exact commit SHA, branch, and dirty/clean status used for analysis to avoid drift confusion.
- When pulling issues, avoid storing credentials; only use tokens from env and redact from logs.
- On very large repos, chunk Git log scanning and stream progress to the TUI/CLI observer to keep UX responsive.
