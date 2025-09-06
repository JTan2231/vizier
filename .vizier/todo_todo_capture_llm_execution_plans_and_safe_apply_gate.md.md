Context:
- High-risk tools (e.g., terraform apply, kubectl apply, destructive shell) should not be executed directly from model outputs. We need the model to emit an explicit plan artifact prior to execution, surface it for human confirmation (TUI/CLI), and persist it in the audit log. This also enables replay, diffing, and offline approval.
- We already have an audit event/journal thread. This builds on that by introducing a first-class "Plan" phase and a gate before side-effecting tool calls.

Deliverables (code-anchored, cohesive):

1) Introduce Plan artifacts and lifecycle
- Files: prompts/src/lib.rs, prompts/src/tools.rs
- Add enum PlanKind { Terraform, Kubectl, Shell, FileWrite, HttpRequest }
- Add struct Plan { id: Uuid, kind: PlanKind, title: String, summary: String, details: String, created_at: DateTime<Utc>, proposed_commands: Vec<String>, requires_confirmation: bool }
- Extend ConversationEvent with PlanProposed { plan: Plan } and PlanDecision { plan_id: Uuid, approved: bool, decided_at, decider: String }
- Provide helper API on Audit: audit.plan_proposed(&Plan), audit.plan_decision(plan_id, approved, decider)

2) Tool contract: plan-before-apply for side-effecting tools
- Files: prompts/src/tools.rs
- For tool traits/impls that can mutate state (terraform, shell exec, file writes):
  - Refactor to expose two phases: fn plan(&self, args) -> Plan and fn apply(&self, plan: &Plan) -> Result<...>
  - apply MUST accept a Plan and verify it matches the intended action (hash proposed_commands to guard against TOCTOU/argument drift). If args->plan changed since approval, reject and emit Error event.
  - Add a default RequiresConfirmation bool on each tool; default true for Terraform/Kubectl/Shell destructive modes; false for read-only.

3) Planner in prompts: force models to output plans instead of direct apply
- Files: prompts/src/walker.rs, prompts/src/lib.rs
- Update prompt construction to instruct the LLM: "For any side-effecting operation, produce a Plan artifact (JSON) with title/summary/details and proposed_commands. Do NOT execute."
- Parse model output: if it emits a Plan JSON block, deserialize to Plan and emit PlanProposed event; otherwise continue as normal for read-only operations.

4) Human gate in TUI
- Files: tui/src/chat.rs
- When a PlanProposed event arrives, open a modal pane showing:
  - Title, summary, details, and a scrollable block with proposed_commands and unified diff if available (e.g., terraform plan output)
  - Key bindings: a=approve, r=reject, v=view full diff, s=save plan to file
- On approve: emit PlanDecision(approved=true) and trigger tool.apply(plan)
- On reject: emit PlanDecision(approved=false) and discard plan; assistant is notified with a standardized message so it can re-plan.
- Persist decisions to audit JSONL; render in the existing audit inspector.

5) CLI non-interactive approval
- Files: cli/src/main.rs, cli/src/config.rs, README.md
- Add subcommands:
  - vizier plan list [--session <id-prefix>] [--kind <...>] [--since <duration>]
  - vizier plan show <plan-id>
  - vizier plan approve <plan-id> [--yes]
  - vizier plan reject <plan-id>
- Implement by reading from the audit store, locating PlanProposed and PlanDecision events. "approve" will enqueue an apply request (see 6).

6) Apply queue and idempotent executor
- Files: prompts/src/lib.rs, prompts/src/tools.rs
- Introduce an ApplyQueue (bounded crossbeam channel) receiving (plan, session_id). The executor thread performs tool.apply(plan) and emits ToolStart/ToolOutput/ToolDone events. This decouples UI from execution and allows CLI approvals to take effect even when TUI is closed (as long as daemon/process is running).
- Ensure at-most-once semantics by including plan.id and rejecting duplicate applies; emit a ToolDone with ok=false and reason="duplicate" for replays.

7) Terraform adapter
- Files: prompts/src/tools.rs, README.md
- Implement a TerraformTool that:
  - plan(args) runs `terraform plan -no-color` in a temp dir or repo dir, captures stdout as details, extracts resources to be changed, and populates proposed_commands=["terraform apply -auto-approve"].
  - apply(plan) runs the exact proposed command sequence; captures stdout/stderr; verifies workspace and directory hash matches the one recorded in Plan details.
- Mark requires_confirmation=true by default.

8) Tests: plan lifecycle, gate enforcement, drift rejection
- Files: prompts/src/lib.rs (tests), tui/src/chat.rs (tests), cli/src/main.rs (tests)
- Cover: a) LLM outputs Plan JSON -> parsed and surfaced; b) Approve triggers apply; c) Reject prevents apply; d) args drift causes rejection; e) duplicate apply ignored; f) Terraform adapter round-trip with a mock.

Notes:
- Plans must be serializable and logged in audit JSONL for replay.
- Keep I/O non-blocking; use background threads for plan parsing and apply execution.
- Security: sanitize displayed commands; never execute arbitrary content unless it came from an approved Plan with matching hash.
- This extends and composes with the Audit Log TODO; reuse its types and sink infrastructure.