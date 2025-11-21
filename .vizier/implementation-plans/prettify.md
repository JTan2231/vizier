---
plan: prettify
branch: draft/prettify
status: draft
created_at: 2025-11-21T15:54:53Z
spec_source: inline
---

## Operator Spec
Today: human epilogues and token/agent summaries print as single-line x=y chains across commands (ask/save/draft/approve/review/merge) and token-usage streams, which are hard to scan.
  Aim: switch to a tight two-column label/value block (padded labels, multi-line, comma-separated numbers) so eyes can read down instead of parsing delimiters.
  Scope: adopt this format for every x=y surface—not just merge epilogues but all outcomes/progress usage lines—while letting verbosity control whether detailed rows (tokens/agent) appear.

## Implementation Plan
## Overview
Reformat Vizier’s human-facing epilogues and token/agent summaries into a readable two-column label/value block across ask/save/draft/approve/review/merge and token-usage progress lines. This improves scanability while respecting the stdout/stderr + verbosity contracts described in the snapshot. The work touches CLI outcome rendering and Auditor token usage reporting without changing core behaviors.

## Execution Plan
1) **Define formatting contract** — Decide the canonical label/value block shape (padded labels, multi-line, comma-separated numbers) and verbosity rules (minimal rows at normal verbosity; token/agent detail only at -v/-vv). Add a shared formatter (likely in `vizier-core/src/display.rs` or a helper consumed by `vizier-cli/src/actions.rs`) that produces newline-joined blocks without ANSI and works in non-TTY output.
2) **Refactor token/agent reporting** — Update `vizier-core/src/auditor.rs` token usage rendering and `vizier-cli/src/actions.rs` helpers (`describe_usage_report`, `describe_usage_totals`, `token_usage_suffix`, `print_token_usage`) to emit the new block format and feed it through progress events so `[usage]` lines print vertically aligned rows at the appropriate verbosity.
3) **Convert command epilogues** — Replace the x=y chains in CLI outcomes with the block formatter: snapshot init outcome, save (`format_save_outcome`), draft (`Draft ready…`/manual flow), approve/review/merge summaries (including CI/CD gate metadata), and any inline ask epilogues. Ensure required fields remain present, optional rows (session, agent, tokens) obey verbosity, and outputs stay on stdout while diagnostic notes stay on stderr.
4) **Adjust progress/history rendering** — If progress events carry multi-line token blocks, ensure `vizier-core/src/display.rs::render_progress_event` cleanly prints them (one line per row, still honoring quiet/progress gating) without breaking existing `[codex]` agent lines.
5) **Docs and tests** — Refresh README/docs snippets if they mention the old x=y style. Update integration/unit tests in `tests/src/lib.rs` (and any UI string asserts) to look for the new block layout instead of x=y substrings; add unit coverage for the formatter if helpful.

## Risks & Unknowns
- Scripts or tests that parse the old x=y strings will break; need to audit coverage to catch them.
- Multi-line progress/events could clutter stderr if not gated correctly; must ensure quiet/non-TTY rules still hold.
- Field selection per verbosity may hide information operators expect; choose defaults carefully to avoid regressions.

## Testing & Verification
- Update and run existing integration tests (`cargo test -p tests`) and relevant unit suites (`cargo test -p vizier-core`, `cargo test -p vizier-cli`) to reflect the new output.
- Spot-check CLI runs (`vizier draft --name prettify …`, `vizier approve … --yes`, `vizier merge … --yes`, `vizier save --no-commit`) under default, -v, and -q to confirm block formatting, proper gating, and absence of ANSI in non-TTY.
- Verify token-usage progress lines show the new layout and respect verbosity (hidden at -q, detailed at -v/-vv).

## Notes
- Keep alignment with the stdout/stderr contract and pending Outcome work; avoid introducing new ad-hoc epilogue shapes outside the shared formatter.

## Amendments (post-implementation)
- Status: implemented/reviewed on the draft branch as of 2025-11-21; not yet merged.
- Delivery summary: Shared block + thousand-separator formatter now lives in `vizier-core/src/display`, progress events render multi-line token blocks, token/agent rows appear in CLI outcomes for ask/save/draft/approve/review/merge and init-snapshot, token usage also prints as a stdout block after agent responses, and `vizier list` intentionally keeps its single-line inventory format.
- Discrepancies vs plan: the original plan gated token/agent rows to `-v/-vv`; the shipped behavior surfaces them at normal verbosity (quiet mode still suppresses). Token-usage blocks emit to stdout in addition to progress events to keep usage visible.
