# Add `vizier approve` to README Core Commands

Thread: Agent workflow orchestration (docs alignment)

Tension
- The feature is shipped (`vizier approve` merges `draft/<slug>` plan branches with a metadata-rich commit), but `README.md` does not list or describe it alongside `vizier draft`. This creates a documentation mismatch that hides an important step in the draft→approve flow.

Desired behavior (Product-level)
- README’s Core Commands section includes a concise entry for `vizier approve` with observable behavior and safe flags (e.g., `--list`, `-y`, `--delete-branch`, `--target`).
- Description matches current behavior (non-FF merge, commit message embeds plan metadata) without over-specifying internal implementation.
- Cross-link deeper docs when present. If `DRAFT.md`/`APPROVE.md` are not yet in-repo, link to `vizier approve --help` and keep copy concise until the docs land.

Acceptance Criteria
- README.md gains a Core Commands bullet for `vizier approve <slug>` that:
  - States it merges `draft/<slug>` back into the primary branch (or `--target`) with a non-FF merge.
  - Mentions `--list` and `-y` at minimum; optional `--delete-branch` and `--target` can be included if space permits.
  - Notes that the merge commit includes plan metadata.
  - Provides a “Learn more” link:
    - Prefer DRAFT.md and APPROVE.md when available;
    - Otherwise, link to `vizier approve --help` as an interim anchor.
- Language is consistent with Snapshot and does not promise unshipped flags or UI.

Pointers
- README.md (Core Commands)
- DRAFT.md, APPROVE.md (integrated draft→approve docs) — missing today; interim link is CLI help
