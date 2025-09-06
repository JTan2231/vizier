Context:
This TODO exists solely to validate the TODO pipeline end-to-end.

Task:
- Add a noop CLI subcommand `vizier sanity` in `cli/src/main.rs` that prints `OK` and exits with code 0.

Acceptance Criteria:
- `cargo run -p cli -- sanity` prints exactly `OK` followed by a newline.
- No changes outside `cli` crate.
- Remove this TODO after validation is complete.