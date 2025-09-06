Goal: Establish a clear stdout/stderr contract across vizier CLI so it behaves like a well-formed Unix tool and becomes observable in shells and supervisors.

Why now: Current CLI prints user-facing guidance, progress spinners/status, LLM content, and token metrics all to stdout via println!. This breaks piping/composability and makes monitoring/alerting noisy. It also hides machine-consumable outputs inside human text.

Acceptance criteria:
- Pipes like `vizier ... | jq`, `> file`, and `2> errors.log` behave predictably.
- Human guidance and ephemeral status go to stderr; structured results go to stdout.
- Non-zero exit codes are used consistently for failure modes.

Code changes (surgical, file/line oriented):
1) cli/src/main.rs
   - print_usage(): send usage/help to stderr instead of stdout. Replace println! with eprintln!.
   - Early no-args branch: after print_usage(), exit(2) (usage) instead of exit(1) (generic error).
   - In --save flow: 
     * Send assistant narrative and token usage to stderr (progress/log), not stdout.
     * Only print the final commit hash or a minimal structured summary to stdout, e.g. JSON: {"committed": true, "message": "..."}.
   - In --summarize flow:
     * Print response.content to stdout only.
     * Send token usage to stderr.
   - In default tool-call flow:
     * Print assistant content to stdout only; token usage to stderr.

2) cli/src/config.rs
   - llm_request and llm_request_with_tools(): change println! side-effects to stderr. Specifically:
     * Within call_with_status, map Status::Working updates to stderr (tui prints to stderr); avoid stray println! newlines to stdout. Replace trailing println!() with eprintln!().
   - After tool-run auto-commit: emit nothing to stdout; if needed, write a concise status line to stderr (e.g. "Committed TODO updates").

3) Add a machine-readable mode flag
   - Args: add `--json` (short -j) boolean.
   - Behavior:
     * When set, stdout emits compact JSON objects for primary results (assistant content, summaries, save outcome).
     * All logs/progress remain on stderr.
   - main.rs branches should gate stdout formatting on this flag.

4) Error handling and exit codes
   - Find panics in main.rs (e.g., unrecognized provider, not in git repo). Replace with user errors to stderr and return with exit codes:
     * 64 (EX_USAGE) for usage errors (unknown provider, bad flags)
     * 69 (EX_UNAVAILABLE) when required environment (git repo) missing
     * 70 (EX_SOFTWARE) for internal errors
   - Map Result errors in async blocks to proper exits; avoid unwrap() for IO/UTF-8 conversions where user input could break.

5) Tests/Manual checks (documented in README.md of cli)
   - Document examples:
     * `vizier --summarize | jq -r '.'` (JSON mode) prints only JSON on stdout.
     * `vizier --chat 2> vizier.log` keeps terminal clean on stdout.
     * `vizier --provider nope` exits 64 with error on stderr.

Non-goals:
- Full TUI refactor; this task only ensures TUI background status uses stderr and doesn’t pollute stdout when not in TUI.

Follow-ups hook for monitoring:
- With a clean contract, supervisors can scrape stderr for operational logs, while stdout feeds downstream tools. Pair with `todo_todo_observable_error_reporting_and_user_event_tracing.md.md` to add structured event emission later.