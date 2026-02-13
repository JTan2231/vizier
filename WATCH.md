# Feature Spec: Live Scheduler Watch Mode (`vizier jobs schedule --watch`)

## Summary
Add an interactive watch mode for scheduler visibility:

- command: `vizier jobs schedule --watch`
- UI style: lightweight `top`-style terminal refresh (no `ratatui`)
- content:
  - top `N` scheduled jobs (default 10)
  - latest output line from the currently running job

This mode is interactive-only and must be rejected when TTY/ANSI requirements are not met.

## Goals
- Provide a single in-place dashboard for queue + running-state awareness.
- Keep implementation lightweight (plain ANSI screen control + existing table rendering).
- Reuse existing scheduler contracts and job record formats.
- Preserve deterministic, script-friendly behavior for non-watch paths.

## Non-Goals
- No curses/rich TUI framework adoption (`ratatui`, etc.).
- No changes to scheduler semantics, locking, retries, or job lifecycle.
- No JSON watch stream in v1.
- No multi-pane scrolling/history browser in v1.

## CLI Contract

### Command
- Extend existing schedule surface:
  - `vizier jobs schedule --watch`

### New Flags (Schedule subcommand)
- `--watch`
  - Enables live interactive rendering loop.
- `--top <N>`
  - Optional, default `10`, minimum `1`.
  - Limits rows shown in the summary table each refresh.
- `--interval-ms <MS>`
  - Optional, default `500`, minimum `100`.
  - Poll interval for refresh.

### Compatibility Rules
- Allowed with:
  - `--all`
  - `--job <JOB>`
  - `--max-depth <N>` (for focused root selection behavior parity)
- `--watch` + `--format dag|json` is invalid.
- `--watch` with `--format summary` is allowed.

## Hard Gating Requirement (TTY + ANSI)
`--watch` must fail fast unless all are true:
- stdout is a TTY
- stderr is a TTY
- ANSI output is enabled (global `--no-ansi` not active)

### Required Error Behavior
- Exit non-zero.
- Emit a direct error message, for example:
  - `` `--watch` requires an interactive TTY with ANSI enabled; rerun without `--watch` for static output. ``

No fallback watch rendering is allowed for non-TTY or no-ANSI contexts.

## UX Specification

### Refresh Layout (Each Tick)
Render full-screen replacement:
1. Header line
   - title + refresh timestamp + interval
2. State summary
   - counts by status buckets (queued/waiting/running/blocked/terminal)
3. Top-N schedule table
   - same fields as summary view:
     - `#`, `Slug`, `Name`, `Status`, `Wait`, `Job`
4. Running job output pane
   - selected running job id (or none)
   - latest log line (stdout/stderr labeled)

### Running Job Selection
- If `--job <JOB>` is set and that job is `running`, use it.
- Otherwise, choose the first visible `running` row in current sort order.
- If no running job exists, pane should display:
  - `No running job`
  - `Latest line: (none)`

### Latest Line Semantics
- Source from selected job’s `stdout.log` and `stderr.log`.
- Determine latest non-empty line from the most recently updated stream.
- Prefix with stream label:
  - `[stdout] ...` or `[stderr] ...`
- If no line exists yet:
  - `(no output yet)`

### Exit Behavior
- v1 minimum: terminate with `Ctrl-C` (SIGINT).
- Optional enhancement: `q` to quit if implemented without adding heavy dependencies.

## Data/Implementation Notes

### Reuse Existing Code
- Job list/graph and summary row builders:
  - `vizier-cli/src/cli/jobs_view.rs`
- Job record + log path helpers:
  - `vizier-cli/src/jobs.rs`
- Table renderer:
  - `vizier-cli/src/actions/shared.rs::format_table`

### New Helper(s)
- Add log tail helper(s) in `vizier-cli/src/jobs.rs`:
  - read trailing bytes from stdout/stderr
  - return latest non-empty line with stream identity
- Keep reads bounded (tail-window, not full file read each tick).

### Rendering Approach
- ANSI clear/redraw per tick:
  - clear screen + cursor home
- Print via stdout only during watch loop.
- Avoid scheduler mutations:
  - watch is read-only
  - no scheduler tick calls
  - no scheduler lock acquisition

## Performance & Safety
- Poll default: 500ms.
- Bound file IO:
  - tail window size (for example 8-16 KiB per stream per tick).
- Handle missing/deleted logs gracefully.
- Handle malformed job records the same way current list/schedule paths do (warn and continue).

## Acceptance Criteria
1. `vizier jobs schedule --watch` renders a repeatedly refreshing in-place dashboard on a TTY with ANSI enabled.
2. Dashboard shows top-N rows from current schedule summary and includes a running-job latest-line pane.
3. `--watch` is rejected when either stdout/stderr is non-TTY.
4. `--watch` is rejected when `--no-ansi` is set.
5. `--watch` with `--format dag|json` fails with explicit incompatibility guidance.
6. Non-watch schedule behavior (`summary`, `dag`, `json`) remains unchanged.

## Test Plan

### Integration Tests (`tests/src/jobs.rs`)
- Reject path: non-TTY environment
  - run `vizier jobs schedule --watch`
  - assert non-zero exit and expected message
- Reject path: `--no-ansi`
  - run `vizier --no-ansi jobs schedule --watch`
  - assert non-zero exit and expected message
- Reject path: incompatible format
  - run `vizier jobs schedule --watch --format json`
  - assert non-zero exit and expected message

### Unit/Focused Tests (CLI module)
- Selection logic for running job (focused job override vs first visible running).
- Latest-line resolver behavior:
  - stdout only
  - stderr only
  - both streams, newest wins
  - empty/missing logs

## Documentation Updates (Implementation Phase)
- Update `docs/user/config-reference.md`:
  - document `vizier jobs schedule --watch`, `--top`, `--interval-ms`
  - call out interactive TTY+ANSI requirement
- Update scheduler observability docs (`docs/dev/scheduler-dag.md`) with watch semantics.
- Ensure help/man output includes new flags and incompatibility constraints.

## Rollout Notes
- Ship as opt-in flag under existing `jobs schedule`.
- No migration impact to existing scripts, since watch is interactive-only and rejected outside TTY+ANSI contexts.
