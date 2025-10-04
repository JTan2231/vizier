Title: Establish stdout/stderr contract, verbosity levels, and TTY-safe status rendering

Narrative link: Terminal-first minimal TUI + renderer-neutral events; Outcome summaries
Depends on snapshot: Running Snapshot — Terminal-minimal output planned; no verbosity controls; spinner emits ANSI to stderr unconditionally

Problem
- CLI emits transient spinner/status like "⠋ Thinking..." to stderr with ANSI control codes even in non-TTY contexts, producing noisy, unconfigurable output.
- stdout is underutilized; users can’t rely on it for stable, parseable results.
- There is no verbosity/quiet control; --debug exists but is not aligned with standard -q/-v conventions.

Desired behavior (product level)
- Output contract:
  - stdout: final, stable results (human-readable outcomes by default; JSON when explicitly requested). For `vizier ask`, stdout contains the assistant’s final answer only (unless --json). For other commands, emit a compact one-line Outcome on success, machine-friendly when possible.
  - stderr: transient status/progress, warnings/errors. No ANSI control sequences unless TTY and verbosity warrants it.
- Verbosity controls:
  - -q/--quiet = errors only (no spinners/status; minimal warnings).
  - -v (info) and -vv (debug) increase detail. Map existing --debug to -vv (kept for compatibility; may deprecate later).
  - --no-ansi or auto-detect: disable ANSI when not a TTY (including CI/pipes).
  - --progress=[auto|never|always] where auto shows spinners only on TTY and when verbosity >= info.
- Status rendering:
  - When enabled, show a spinner/status on stderr that is cleanly erased and never leaks control codes into logs/pipes. In non-TTY or quiet mode, suppress the spinner entirely; optionally print a single "Working: <msg>" line once at start if verbosity >= info and --progress=always.

Acceptance criteria
- Non-TTY run (e.g., `vizier ask foo | cat`) produces no ANSI sequences at all; stdout contains only the final answer text (or JSON when -j/--json). stderr contains nothing but errors/warnings.
- TTY run with default verbosity: transient spinner/status appears on stderr and is cleared on completion; stdout contains only final answer/outcome. No residual "[1G\u001b[2K" fragments.
- With -q: no spinner/status, only final stdout result; stderr restricted to errors.
- With -v/-vv: stderr includes additional progress lines (noisy details at -vv); spinner persists only if --progress=always and TTY; otherwise line-oriented progress.
- `vizier save`/`clean`/`snapshot init` print a one-line Outcome to stdout on success (e.g., "Saved conversation <hash>; 3 TODOs updated"), matching Auditor/VCS facts.

Anchors
- vizier-core/src/display.rs (status renderer)
- vizier-cli/src/main.rs (global flags, TTY detection)
- vizier-core/src/auditor.rs (stdout/stderr usage; outcome prints)

Notes
- Keep implementation open: adapter in `display` should accept a config describing TTY, progress mode, verbosity, and ANSI enabled; CLI populates it from flags + IsTerminal detection.
- Preserve current behavior for Chat TUI (alt screen) unaffected; this task concerns CLI line-oriented surfaces.
