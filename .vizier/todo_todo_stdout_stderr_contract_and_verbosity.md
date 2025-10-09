Thread: Stdout/stderr contract + verbosity — advancing with acceptance + test matrix and anchors

Tension (observed in code state)
- Stderr emits ANSI cursor controls unconditionally via vizier-core::display, leaking sequences in logs and non-TTY.
- Stdout lacks a stable, final Outcome line/JSON; scripts cannot reliably consume results.
- No -q/-v/-vv controls; progress overwhelms human output; protocols are undefined.

Behavioral goals
- TTY-gated progress: Only show spinners/status with ANSI when stderr is a TTY. In non-TTY, collapse to minimal plain-text progress or suppress based on verbosity.
- Verbosity levers: -q suppresses human epilogues and progress (errors only); default prints compact human epilogue; -v/-vv increase diagnostics on stderr without polluting stdout.
- Stable stdout result: Always emit a final Outcome on stdout — human block by default; exact JSON when --json or in protocol mode. No ANSI on stdout ever.

Acceptance criteria
1) TTY gating
   - Non-TTY: no ANSI sequences are emitted on either stream; progress lines are suppressed unless -v/-vv and even then remain plain text.
   - TTY: progress spinners/status may use ANSI on stderr only; stdout remains clean.
2) Verbosity flags
   - -q/--quiet: suppress progress and human epilogue; still print JSON with --json; exit codes remain informative.
   - default: minimal progress; compact human epilogue to stdout.
   - -v: more granular progress to stderr; warnings surfaced.
   - -vv: debug-grade progress to stderr; include timing breakdowns.
3) Outcome on stdout
   - Human epilogue by default; matches Auditor/VCS facts.
   - When --json, print a single JSON object conforming to outcome.v1 (see Outcome TODO) as the only stdout payload.
   - In protocol mode, emit NDJSON events and a final outcome.v1 object; no human prose.
4) Flags/config mapping
   - CLI flags map to config keys; config can set defaults; flags override.
5) Tests (integration)
   - Matrix: {TTY, non-TTY} x {quiet, default, -v, -vv} x {--json?} across chat and save flows.
   - Assert: no ANSI in non-TTY; stdout contains exactly one epilogue or one JSON object; stderr contains progress per verbosity; exit codes stable.

Pointer-level anchors
- vizier-core/src/display.rs: centralize TTY detection and verbosity policy; provide helpers for progress vs outcome rendering.
- vizier-cli/src/main.rs and actions.rs: wire -q/-v/-vv, --json; ensure stdout reserved for final outcome.
- tests/src/main.rs: harness to simulate TTY vs non-TTY; capture streams; assert contracts.

Cross-links
- Outcome summaries TODO: defines outcome.v1 schema consumed here when --json.
- Mode split TODO: protocol mode tightens IO guarantees derived from these rules.

Safeguards
- Never emit ANSI to stdout.
- In --json/protocol modes, stderr may still carry progress unless -q; document for callers to ignore stderr.

Open trade space
- Whether to allow colorized human epilogue at -v/-vv; default is plain for readability. Keep configurable.


---

