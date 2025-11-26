# Snapshot bootstrap for existing projects and `vizier init-snapshot`

Thread: Snapshot bootstrap ergonomics (`vizier init-snapshot`) — cross: Narrative storage, Snapshot-first prompting, Configuration posture + defaults

## Tension
- `vizier init-snapshot` is the surviving bootstrap command, but dropping it into an already-active repo can feel like a demand to summarize the entire project history before agents are “allowed” to participate.
- That expectation makes adoption heavy: operators hesitate to run init-snapshot on mature projects because they don’t have a clear, scoped way to describe what already happened versus what should happen next.
- The current README positioning uses `vizier init-snapshot` as the Hello World step, which reinforces the sense that existing projects are “behind” if they lack a fully curated snapshot.

## Desired behavior (product-level)
- Treat `vizier init-snapshot` as a forward-looking framing tool, not a historical audit requirement:
  - New repositories and fresh features can opt into a richer initial snapshot when it helps clarify intent.
  - Existing projects are explicitly encouraged to “start where they are” with a minimal snapshot that focuses on current behavior and near-term threads rather than reconstructing the full past.
- Snapshot bootstrapping feels lightweight:
  - Operators understand that they can seed the snapshot with just a few key themes and active tensions, and let Default-Action Posture evolve it over time.
  - There is no implied obligation to backfill all prior context before using `vizier ask/save/draft/approve/review/merge`.
- Documentation and examples match this posture:
  - README and workflow docs describe init-snapshot as an optional bootstrap for clarifying today’s story, especially on new projects/features.
  - Existing-repo examples call out incremental adoption paths (“add a snapshot slice for the feature you’re about to touch”) instead of suggesting a full-project rewrite.

## Acceptance criteria
- README and any quick-start material:
  - Emphasize that `vizier init-snapshot` is helpful but optional; existing repos can begin with a small, present-focused snapshot without summarizing all prior work.
  - Include at least one example where an operator runs init-snapshot specifically for a new feature or subsystem in a larger, pre-existing project.
- Snapshot guidance (prompts + docs):
  - Makes clear that the snapshot is a living document that can start shallow and grow; agents are instructed to evolve it forward rather than retroactively re-narrating the entire codebase.
  - Reinforces that it is acceptable to leave historical gaps as long as current behaviors, tensions, and upcoming work are captured.
- Operator experience:
  - Teams adopting Vizier in an existing repo can run a minimal init-snapshot (or rely on Default-Action Posture to grow the snapshot) without feeling blocked by missing history.
  - Conversations about older, undocumented behavior naturally spawn new snapshot slices for that behavior instead of pressuring operators to rebuild a full historical arc.

## Status
- `vizier init-snapshot` remains the bootstrap command and is positioned as the entrypoint in README quick-start examples.
- This thread tracks the repositioning of init-snapshot toward a “start where you are” posture for existing projects and a richer-but-optional bootstrap for new repos/features, plus the documentation updates and examples needed to make that behavior obvious.
