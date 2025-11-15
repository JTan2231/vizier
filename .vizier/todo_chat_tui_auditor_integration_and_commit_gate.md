# Chat path retired — CLI-only posture (Thread: Commit isolation + gates)

## Resolution
- The `vizier chat` subcommand and its alt-screen UI were removed; operators now drive all narrative updates through single-shot CLI flows (`vizier ask`, `vizier save`, `vizier draft/approve/merge`).
- Chat/editor-specific prompts, config entries, and pending-commit confirmation detours were deleted, so no assistant action depends on a bespoke TUI loop.
- Commit messages flow through the existing Auditor + Pending Commit gate automatically, and `$EDITOR` remains the only opt-in editing surface for commit text.

## Narrative impact
- Commit isolation stays intact because every assistant-initiated change still stages through the Auditor gate; nothing can bypass it via a conversational surface.
- Outcome summaries, session logging, and DAP continue to operate via the CLI event stream; there is no parallel UI contract to maintain.
- Multi-agent orchestration threads now focus solely on CLI workflows and Codex-backed plan branches rather than UI parity.

## Follow-ups
- Keep pushing Outcome/JSON parity and session logging so CLI-first remains auditable (see related TODOs under Outcome summaries + Session logging).
- Ensure docs and prompts describe the CLI-only workflow so operators aren’t looking for the removed chat interface.
