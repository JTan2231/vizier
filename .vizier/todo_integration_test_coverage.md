Thread: Integration test infrastructure → Anchors: see SNAPSHOT “Anchors” section.

Goal: Extend end-to-end coverage to guard the new control levers, history/revert, and auditor behaviors as they land.

Behavior-first scope
- Config-to-CLI-to-prompt: Running `vizier --temperature 0.7 --history-limit 3 --non-interactive --no-auto-commit` results in the system prompt including `<config>` with those values and enforces non-interactive safety (destructive ops blocked unless allowlisted).
- Auditor staging isolation: When there are existing staged changes outside `.vizier/`, a conversation commit only includes conversation changes; prior staged set is restored afterward.
- History and revert: After applying a model-proposed patch, an Operation is recorded; invoking revert restores the pre-op state without stray files. Prefer patch-based revert; fall back to VCS if needed.
- TUI smoke/headless hooks: In CI/headless, a minimal run exercises chat flow and diff rendering hooks or a headless equivalent, ensuring no panics and that token-stream callbacks are invoked at least once.

Acceptance criteria
1) Tests pass on CI via `integration-tests.yml` and on local runs `cargo test -p tests`.
2) Failures clearly identify which behavior regressed (config mapping, auditor isolation, history/revert, or TUI hooks).
3) Temp repos simulate A/M/D/R cases; assertions cover staged vs unstaged boundaries.
4) No network secrets required; tests skip network-bound calls behind a feature flag or mock when running in CI.

Pointers
- tests/src/main.rs for orchestration; vizier-cli/src/main.rs (flags), vizier-core/src/{config.rs,auditor.rs,vcs.rs,tools.rs}, vizier-tui/src/chat.rs for surfaces under test.[2025-09-20] Trim coverage to near-term surfaces.

Keep:
- Config→CLI→prompt mapping assertions for the essential levers.
- Auditor staged-set isolation around conversation commits (A/M/D/R).
- Commit gate flows: CLI interactive (editor proposal), CLI non-interactive (--yes + message), and a TUI/headless smoke that verifies the gate is presented or refused appropriately.

Defer:
- Full history ring buffer and complex revert scenarios; keep only revert-last success case once history lands.
- Streaming token/event timeline hooks.
- Narrative contract/drift enforcement.

Acceptance remains: tests run in CI and locally; failures point to the specific behavior (config mapping, staged isolation, commit gate, revert-last once available).


---

