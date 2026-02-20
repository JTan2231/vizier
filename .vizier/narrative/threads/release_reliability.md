# Release reliability

Status (2026-02-20): ACTIVE.

Thread: Release reliability (cross: configuration posture, release docs, man-page drift gate)

Tension
- Release automation often needs the final release commit/tag metadata available to publish or deploy scripts.
- If those scripts fail after local release mutations, operators need deterministic rollback for local Git state.

Desired behavior (Product-level)
- `vizier release` may run an optional release gate script after release commit/tag creation.
- Script resolution must be deterministic across config and CLI override surfaces.
- Script failure must fail the release and roll back local release mutations created by that run.
- `vizier release --dry-run` remains script-free and mutation-free.

Acceptance criteria
- Script precedence is `--no-release-script` > `--release-script <cmd>` > `[release.gate].script` > none.
- Script receives `VIZIER_RELEASE_VERSION`, `VIZIER_RELEASE_TAG`, `VIZIER_RELEASE_COMMIT`, and `VIZIER_RELEASE_RANGE`.
- On script failure, rollback deletes the created tag (if any), resets the original branch to the recorded start commit, and restores index/worktree state to that commit.
- Help/docs/man/config references describe release gate config, CLI flags, rollback behavior, and dry-run skip semantics.

Status
- Implemented in `vizier-cli/src/actions/release.rs` with transactional rollback for post-mutation script failures.
- Config surface now includes `[release.gate].script`; CLI surface now includes `--release-script` and `--no-release-script`.
- Integration coverage now includes scripted success, scripted failure rollback, `--no-tag` env behavior, script override/suppression precedence, and dry-run script skip behavior.

Update (2026-02-20)
- Release output now reports script invocation identity/status and rollback outcome details, including explicit recovery hints when rollback is incomplete.
