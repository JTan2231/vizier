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

Add integration tests for config mapping, commit isolation, and commit gates (defer full history; cover revert-last when available).
Description:
- Verify CLI flags map to Config and appear in the prompt <config> block; enforce non-interactive safety (destructive ops require explicit consent). (snapshot: Next moves 1–3,5)
- Guard auditor’s staged-set isolation: conversation commits touch only .vizier paths; pre-existing staged A/M/D/R changes remain staged and are excluded, then restored. (thread: Commit isolation + gates)
- Exercise commit gate flows:
  • CLI interactive: opens proposal in $EDITOR; empty or “# abort” cancels with no changes.
  • CLI non-interactive: requires --yes and a commit message; otherwise refuses.
  • TUI/headless smoke: minimal path that ensures the gate is presented/refused without panics.
- Defer full history ring buffer; once revert(n=1) lands, add a happy-path revert-last test.

Acceptance Criteria:
- `cargo test -p tests` and CI workflow (integration-tests.yml) run and pass.
- Failing assertions clearly indicate which behavior regressed: config→CLI→prompt mapping; staged-set isolation (A/M/D/R); commit gate flows; revert-last (when implemented).
- Temp repos simulate adds, modifies, deletes, and renames; assertions verify staged vs unstaged boundaries before/after conversation commits.
- Tests require no network secrets; network-bound calls are mocked or skipped via feature flags in CI.

Pointers:
- tests/src/main.rs (orchestration)
- vizier-cli/src/main.rs (flag parsing, editor launch)
- vizier-core/src/{config.rs,auditor.rs,vcs.rs} (mapping, isolation)
- vizier-core/src/history.rs (revert-last once available)
- vizier-core/src/tools.rs
- vizier-tui/src/chat.rs (gate presentation; headless smoke)

Implementation Notes (safety/correctness):
- Use deterministic temp repos; ensure editor invocation is stubbed in tests to avoid real editors.
- For rename coverage, assert A/M/D/R handling matches snapshot expectations exactly. (thread: Integration tests; snapshot: Integration tests — active)