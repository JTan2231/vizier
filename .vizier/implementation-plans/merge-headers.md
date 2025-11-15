---
plan: merge-headers
branch: draft/merge-headers
status: draft
created_at: 2025-11-15T17:23:11Z
spec_source: file:draft
---

## Operator Spec
i think we can clean up how merge messages are headed

here's an example
```
commit ca95a3d369fee7d44de4d74f43d688a065428236 (HEAD -> master)
Merge: 7629c3c 04c8a01
Author: Joey Tan <j.tan2231@gmail.com>
Date:   Sat Nov 15 11:16:49 2025 -0600

    feat: merge plan convo

    Plan: convo
    Branch: draft/convo
    Summary: we need to stop recording conversations on the commit history. conversations no longer have a place in the commit history.
    Status: implemented
    Spec source: inline

    Plan Document:
    ---
    plan: convo
    branch: draft/convo
    status: implemented
    created_at: 2025-11-15T16:35:43Z
    spec_source: inline
    implemented_at: 2025-11-15T17:02:32Z
```
there's obvious redundancy, of course--but really i don't think we need any of this.

a simple header like:
```
feat: ...
Implementation Plan:
...
```
where that final ellipsis is the actual doc contents

## Implementation Plan
## Overview
`vizier merge` currently generates merge commits whose bodies restate the plan slug, branch, summary, status, and spec source before inlining the plan document. Operators find that redundant because the structured plan already embeds that metadata in its front matter. This change simplifies the merge commit message to a single subject line plus an `Implementation Plan:` block that contains the stored plan document (and any optional operator note). The users impacted are anyone reviewing plan merges or consuming the conflict-sentinel metadata, and the work is needed now so merge commits stay concise and easier to scan without losing the embedded plan doc that auditors rely on.

## Execution Plan
1. **Define and implement the new merge commit template**
   - Update `vizier-cli/src/actions.rs::build_merge_commit_message` so the subject stays `feat: merge plan <slug>` but the body only includes:
     1. An optional note section when `--note` is provided (e.g., `Operator Note: ...`).
     2. An `Implementation Plan:` header followed by the trimmed plan document.
   - Drop the current `Plan:/Branch:/Summary:/Status:/Spec source:` block entirely; rely on the plan doc’s front matter for that data.
   - Ensure the helper still handles edge cases: when `plan_document` is `None`, emit a short placeholder like `Implementation plan document unavailable for <slug>` instead of leaving an empty commit body; keep whitespace predictable so resumes and editors don’t introduce noise.
   - Verify that the builder is still used everywhere (`prepare_merge`, conflict sentinels, auto-resolve paths) so the new layout applies uniformly; add inline docs/comments to capture the new contract.

2. **Regressions and unit coverage for the new formatter**
   - Add or update a focused unit test in `vizier-cli/src/actions.rs` (or a nearby test module) that exercises `build_merge_commit_message` with and without `plan_document`/`--note`, asserting the final string uses the new structure and never reintroduces the removed headers.
   - Grep for `"Plan:"`, `"Branch:"`, `"Spec source"`, and similar patterns across `vizier-cli/` to ensure no other code paths try to parse or emit the old labels. Clean up any stray format expectations uncovered during this sweep (e.g., logging or display code that might echo the same block).

3. **Refresh integration tests and fixtures**
   - Update `tests/src/main.rs::test_approve_merges_plan` to stop looking for `"Plan: <slug>"` in the merge commit and instead assert that the message contains the `Implementation Plan:` header and the plan front matter it expects.
   - Review other integration tests (`tests/src/lib.rs::test_merge_removes_plan_document`, conflict sentinel assertions, etc.) to ensure their string checks align with the simplified commit body; if any tests depend on the removed labels, rewrite their expectations to validate the presence of the implementation plan block or the operator note when applicable.
   - Verify the mock data under `draft/` (used for documentation/examples) no longer references the old header format; regenerate or edit those snapshots if they’re meant to mirror real commits.

4. **Document the new merge-message behavior**
   - Revise `docs/workflows/draft-approve-merge.md` (and any other docs that describe merge metadata) so the “Merge mechanics” section states that the commit subject is `feat: merge plan <slug>` and the body contains an `Implementation Plan:` block (plus optional operator note), replacing the paragraph that lists every duplicated field.
   - Scan README/AGENTS.md or other operator-facing guides for references to “Plan/Branch/Summary” lines in merge commits; update those callouts so they describe the simplified format and still mention that the full plan document is embedded for auditability.
   - Call out the change in the documentation as a clarity/UX improvement so reviewers know why legacy commits look different.

## Risks & Unknowns
- Commit message parsing: any downstream tooling that scraped `Plan:` or `Summary:` headers will break; we don’t know whether such scripts exist. The mitigation is to highlight the new format in docs/release notes and emphasize that the plan doc still carries those fields.
- `--note` placement: the spec still allows notes, but the new layout needs a clear spot that doesn’t get confused with the plan front matter. We should confirm whether “Operator Note” before the plan block meets expectations.
- Large commit messages: embedding only the plan doc keeps the body long; trimming other metadata might not materially shorten it. There’s no immediate mitigation besides trimming leading/trailing whitespace, but we should watch for Git limits.

## Testing & Verification
- Unit tests:
  - Add a direct `build_merge_commit_message` test covering (a) base case with a plan doc, (b) missing plan doc, and (c) notes, ensuring the old headers never appear.
- Integration tests via the `tests` crate:
  - `cargo test -p tests test_approve_merges_plan` (or the full suite) should assert the merge commit body only mentions `Implementation Plan` plus the expected plan content.
  - `cargo test -p tests test_merge_removes_plan_document` already checks for `Implementation Plan`; confirm it still passes after expectations change elsewhere.
  - Run the conflict auto-resolution/resume tests to ensure the stored `merge_message` inside `.vizier/tmp/merge-conflicts/*.json` reflects the new format, since they indirectly validate commit creation.
- Manual spot check:
  - Execute `vizier merge <slug>` in a sandbox repo, open the merge commit with `git show`, and confirm the body matches the new layout (subject + optional note + plan doc) without any `Plan:`/`Branch:` headers.

## Notes
- Coordinate with anyone consuming `vizier merge` output (e.g., automation around release notes) so they anticipate the streamlined commit body.
