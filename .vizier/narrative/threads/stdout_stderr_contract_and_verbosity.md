Canonicalization note (2025-10-06): This is the canonical thread for the stdout/stderr contract and verbosity work. Legacy TODO files were removed; keep cross-links and acceptance anchored here.

---

Thread: Stdout/stderr contract + verbosity (cross: Outcome summaries)

Snapshot anchor
- Active threads — Stdout/stderr contract + verbosity (Running Snapshot — updated).
- Code state — Display/verbosity and stdout usage bullets (Running Snapshot — updated).

Problem/Tension
- Prior behavior leaked ANSI spinner/status to logs and non-TTY contexts and lacked consistent verbosity controls; stdout carried no stable, scriptable outcome.
- Current state (partial): CLI ships -q/-v/-vv and --no-ansi, spinner rendering has been removed so progress is always line-based, but commands still print ad-hoc outcomes to stdout and there’s no standardized outcome.v1 JSON or uniform human epilogue across actions.

Desired behavior (Product-level)
- Honor a strict IO contract:
  - Non-TTY: never emit ANSI; stderr carries only errors/warnings per verbosity; stdout carries a single, stable Outcome (human or JSON).
  - TTY: progress history remains line-based per verbosity (no spinner), and the final Outcome always lands on stdout.
- Provide levers: -q suppresses non-error output; -v/-vv increase detail on stderr; --no-ansi disables ANSI even on TTY.
- Standardize Outcome: every action (ask, chat step, save, init, draft/approve) emits the same compact epilogue; with --json emit outcome.v1 on stdout.

Acceptance criteria
1) Verbosity levers:
   - -q emits only errors to stderr and the minimal Outcome on stdout.
   - -v shows Info; -vv shows Debug; --no-ansi strips ANSI even on TTY.
2) TTY gating:
   - Non-TTY never writes ANSI sequences; progress stays line-oriented; Outcome still appears on stdout.
3) Outcome standardization:
   - ask/save/init/draft/approve/merge all print a one-line human Outcome on stdout by default; with --json print outcome.v1 JSON only (no human text).
   - Fields cover {action, elapsed_ms, changes:{A,M,D,R,lines}, commits:{conversation,.vizier,code}, gates:{state,reason}, token_usage?, session.path?}.
4) Tests: matrix across (TTY vs non-TTY) × (quiet/default/-v/-vv) asserting no ANSI in non-TTY, stable presence/shape of Outcome, and correct gating of the line-based progress history.

Status update (2025-11-15, revised 2025-11-30)
- Shipped: -q/-v/-vv, --no-ansi, and spinner removal (vizier-core/src/display.rs now only emits line-based history; vizier-cli dropped `--progress`).
- Outstanding: unify stdout Outcome across commands; implement outcome.v1 schema; ensure non-TTY never sees ANSI and stderr respects verbosity in all paths.
- Manual `vizier clean` epilogues were removed entirely; TODO hygiene now flows through Default-Action Posture plus the dedicated GC work tracked in the snapshot.
Update (2026-01-27)
- Background-by-default now prints a multi-line `Outcome: Background job started` block for detached runs; treat this as a temporary stdout contract exception until the unified Outcome epilogue/JSON lands.
Update (2026-01-30)
- Background job finalization now flushes stdout/stderr before marking jobs complete so `vizier jobs tail --follow` reliably captures the final assistant output.

Pointers
- vizier-cli/src/main.rs (global flags → display config)
- vizier-core/src/display.rs (TTY gating, verbosity)
- vizier-cli/src/actions.rs (current ad-hoc outcomes for save/init)
- Cross-link: Outcome summaries thread (see snapshot)
