# README: Document draft → approve → merge flow (correct semantics)

Thread: Agent workflow orchestration (docs alignment)

Tension
- CLI ships a two-step flow: `vizier approve` implements the plan on the draft branch, and `vizier merge` performs the non‑FF merge back onto the primary. README now mentions both, but copy/flags must match actual behavior to avoid confusion left over from the earlier one-step approval.

Desired behavior (Product-level)
- README Core Commands concisely document both commands with observable outcomes and correct flags:
  - `vizier approve <plan>`: implements plan on `draft/<plan>` using Codex. Flags: `--list`, `-y/--yes`, `--target`, `--branch`.
  - `vizier merge <plan>`: merges `draft/<plan>` to primary (or `--target`) with a metadata‑rich non‑FF commit. Flags: `-y/--yes`, `--keep-branch` (default is to delete the draft branch; `--delete-branch` remains as a deprecated alias), `--target`, `--branch`, `--note`, `--auto-resolve-conflicts`.
- Each entry states that the merge commit embeds plan metadata (plan, branch, status, spec source, created_at, summary).
- Provide “Learn more” anchors to `vizier approve --help` and `vizier merge --help` until DRAFT.md/APPROVE.md land.

Acceptance Criteria
- README.md Core Commands show both entries with accurate behavior and flag sets; no claim suggests that `approve` performs the Git merge.
- Language matches current CLI and Snapshot; no unshipped flags/UI.
- If deeper docs appear (DRAFT.md/APPROVE.md), README links to them; otherwise links to `--help` are present.

Status
- README Core Commands updated to list accurate flags for `approve` and `merge`, including the new default-branch-deletion behavior (`--keep-branch` to retain, `--delete-branch` as a deprecated alias) and `--auto-resolve-conflicts`. Verified against `vizier-cli/src/main.rs` CLI definitions for Approve/Merge and `docs/workflows/draft-approve-merge.md`; acceptance criteria met—this thread can be treated as closed and now also documents the default deletion posture.

Pointers
- README.md (Core Commands)
- docs/workflows/draft-approve-merge.md (end-to-end draft→approve→merge choreography); future DRAFT.md/APPROVE.md docs can layer on top if needed
