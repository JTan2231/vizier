Update (2025-10-04): CLI-first delivery; JSON schema named outcome.v1; explicit stdout ownership.

- Scope: Deliver standardized Outcome epilogue on CLI now; TUI panel deferred until a UI surface exists.
- Schema: outcome.v1 { action, elapsed_ms, model, changes:{A,M,D,R,lines_added,lines_removed,hunks}, commits:{conversation, dot_vizier, code, shas:[]}, gates:{state, reason}, branch, pr_url, next_steps:[...] }.
- Output contract: Human epilogue printed to stdout unless --quiet; JSON available with --json; NDJSON with --json-stream. No ANSI in non-TTY.
- Tests: Add fixtures to integration_test_coverage asserting schema validity and human epilogue wording for: no changes; pending gate; auto-commit; rejected; error.
- Cross-link: stdout/stderr contract TODO governs rendering rules; Agent Basic Command TODO defines branch/pr fields.

---

