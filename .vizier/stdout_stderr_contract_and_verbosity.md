Update (2025-10-04): Scope, acceptance, and cross-links tightened per Snapshot threads: Terminal-minimal, Outcome summaries, Integration tests.

Problem (evidence):
- CLI currently leaks ANSI control sequences to logs/non-TTY (vizier-core::display). stdout/stderr roles are inconsistent; outcomes not reliably on stdout.

Desired behavior (product-level):
- Respect TTY detection: emit progress/ANSI only when stderr is a TTY. In non-TTY, suppress control codes entirely.
- Provide verbosity levers: -q/--quiet (suppress human epilogues; errors only), -v/-vv (increase diagnostic detail on stderr); default is concise with an Outcome line on stdout.
- Standardize where the final Outcome appears: machine-trustworthy line/block on stdout, mirrored by assistant epilogue.

Acceptance criteria:
1) Non-TTY runs emit zero ANSI/control sequences; stdout carries a single-line or compact Outcome; stderr contains only errors/warnings unless -v/-vv.
2) TTY runs: status/spinner may render on stderr unless -q; final Outcome printed to stdout regardless of TTY.
3) Flags map to config (vizier.toml): quiet, verbosity, ansi.force, ansi.never. CLI flags override config.
4) JSON modes: `--json` prints only a single JSON object with outcome.v1 schema on stdout; `--json-stream` prints NDJSON events (status/outcome) to stdout with no ANSI. In both, stderr follows verbosity rules.
5) Tests: matrix across TTY vs non-TTY, quiet/verbose levels, human vs json vs json-stream; assert no ANSI in non-TTY and presence of Outcome on stdout.

Trade space/notes:
- Keep renderer-neutral: progress/status modeled as events; CLI renders them conditionally. No fullscreen/alt-screen.
- Safety: ensure Outcome is computed after writes and before exit; avoid partial prints on failure.

Cross-links:
- Snapshot threads: Terminal-minimal, Outcome summaries, Integration tests.
- Pointer anchors: vizier-core::display, vizier-cli/src/actions.rs, vizier-core/src/auditor.rs.

---

Update (2025-10-04): Scope, acceptance, and cross-links tightened per Snapshot threads (Terminal-minimal, Outcome summaries, Integration tests).

Problem (evidence):
- CLI currently leaks ANSI control sequences to logs/non-TTY (vizier-core::display). stdout/stderr roles are inconsistent; outcomes not reliably on stdout.

Desired behavior (product-level):
- Respect TTY detection: emit progress/ANSI only when stderr is a TTY. In non-TTY, suppress control codes entirely.
- Provide verbosity levers: -q/--quiet (suppress human epilogues; errors only), -v/-vv (increase diagnostic detail on stderr); default is concise with an Outcome line on stdout.
- Standardize where the final Outcome appears: machine-trustworthy line/block on stdout, mirrored by assistant epilogue.

Acceptance criteria:
1) Non-TTY runs emit zero ANSI/control sequences; stdout carries a single-line or compact Outcome; stderr contains only errors/warnings unless -v/-vv.
2) TTY runs: status/spinner may render on stderr unless -q; final Outcome printed to stdout regardless of TTY.
3) Flags map to config (vizier.toml): quiet, verbosity, ansi.force, ansi.never. CLI flags override config.
4) JSON modes: `--json` prints only a single JSON object with outcome.v1 schema on stdout; `--json-stream` prints NDJSON events (status/outcome) to stdout with no ANSI. In both, stderr follows verbosity rules.
5) Tests: matrix across TTY vs non-TTY, quiet/verbose levels, human vs json vs json-stream; assert no ANSI in non-TTY and presence of Outcome on stdout.

Trade space/notes:
- Keep renderer-neutral: progress/status modeled as events; CLI renders them conditionally. No fullscreen/alt-screen.
- Safety: ensure Outcome is computed after writes and before exit; avoid partial prints on failure.

Cross-links:
- Snapshot threads: Terminal-minimal, Outcome summaries, Integration tests.
- Anchors: vizier-core::display, vizier-cli/src/actions.rs, vizier-core/src/auditor.rs.

---

