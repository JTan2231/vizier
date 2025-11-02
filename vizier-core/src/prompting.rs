use std::io;
use std::path::PathBuf;

use crate::tools;

const PROMPT_RELATIVE_DIR: &str = "docs/prompting";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptScope {
    ArchitectureOverview,
    SubsystemDetail,
    InterfaceSummary,
    InvariantCapture,
    OperationalThread,
}

impl PromptScope {
    pub fn file_name(self) -> &'static str {
        match self {
            PromptScope::ArchitectureOverview => "architecture_overview.md",
            PromptScope::SubsystemDetail => "subsystem_detail.md",
            PromptScope::InterfaceSummary => "interface_summary.md",
            PromptScope::InvariantCapture => "invariant_capture.md",
            PromptScope::OperationalThread => "operational_thread.md",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PromptScope::ArchitectureOverview => "Architecture Overview",
            PromptScope::SubsystemDetail => "Subsystem Detail",
            PromptScope::InterfaceSummary => "Interface or API Summary",
            PromptScope::InvariantCapture => "Invariant Capture",
            PromptScope::OperationalThread => "Operational Thread",
        }
    }

    pub fn default_template(self) -> &'static str {
        match self {
            PromptScope::ArchitectureOverview => ARCHITECTURE_OVERVIEW_DEFAULT,
            PromptScope::SubsystemDetail => SUBSYSTEM_DETAIL_DEFAULT,
            PromptScope::InterfaceSummary => INTERFACE_SUMMARY_DEFAULT,
            PromptScope::InvariantCapture => INVARIANT_CAPTURE_DEFAULT,
            PromptScope::OperationalThread => OPERATIONAL_THREAD_DEFAULT,
        }
    }
}

pub fn all_scopes() -> &'static [PromptScope] {
    &[
        PromptScope::ArchitectureOverview,
        PromptScope::SubsystemDetail,
        PromptScope::InterfaceSummary,
        PromptScope::InvariantCapture,
        PromptScope::OperationalThread,
    ]
}

pub fn prompt_directory() -> PathBuf {
    let mut base = PathBuf::from(tools::get_todo_dir());
    base.push(PROMPT_RELATIVE_DIR);
    base
}

pub fn ensure_prompt_directory() -> io::Result<()> {
    std::fs::create_dir_all(prompt_directory())
}

pub fn load_prompt(scope: PromptScope) -> io::Result<String> {
    let custom_path = prompt_directory().join(scope.file_name());
    match std::fs::read_to_string(&custom_path) {
        Ok(contents) => Ok(contents),
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                Ok(scope.default_template().to_string())
            } else {
                Err(err)
            }
        }
    }
}

pub fn scaffold_prompt(scope: PromptScope) -> io::Result<PathBuf> {
    ensure_prompt_directory()?;
    let destination = prompt_directory().join(scope.file_name());
    if destination.exists() {
        return Ok(destination);
    }

    std::fs::write(&destination, scope.default_template())?;
    Ok(destination)
}

const ARCHITECTURE_OVERVIEW_DEFAULT: &str = r#"
# project_architecture_overview: Project Architecture Overview

**Purpose:**
Provide a cohesive picture of the entire codebase—its major subsystems, shared foundations, and interaction patterns—so maintainers can understand how the system coheres end to end.

**Context:**
- **Composition:** Identify the major crates, services, libraries, and supporting tools that make up the project.
- **Boundaries:** Describe how responsibilities are partitioned and where interfaces or data flows cross between them.
- **Flow:** Trace typical high-level control and data movement through the system (user requests, background jobs, batch flows, etc.).
- **Cross-Cutting Concerns:** Explain how logging, persistence, authentication, configuration, or deployment frameworks weave through multiple layers.
- **Constraints:** Capture global invariants, performance ceilings, or operational rules that shape the architecture.

**Prompt Guidance:**
Summarize what the project is built from, how its parts cooperate, and what global patterns (dependency, layering, data flow) emerge.
Ground every statement in concrete evidence: module structure, config files, or test coverage.
Avoid conjecture—record only what the current repository and docs substantiate.

**Evidence Checklist:**
- Entry points: binaries, services, CLI targets, or public APIs.
- Core contracts: cross-crate traits, RPC/REST boundaries, schema definitions.
- Tests: integration suites or e2e harnesses proving subsystem relationships.
"#;

const SUBSYSTEM_DETAIL_DEFAULT: &str = r#"# project_subsystem_atlas: Project Subsystem Atlas

**Purpose:**
Enumerate each significant subsystem within the project, describing its ownership boundaries, dependencies, and operational guarantees in a project-wide context.

**Context:**
- **Topology:** List directories, crates, or modules representing distinct domains or functions.
- **Entry Points:** Summarize how external users or internal components engage each subsystem.
- **Dependencies:** Outline how subsystems depend on one another or on shared infrastructure.
- **Verification:** Identify tests or CI checks ensuring subsystem integrity.

**Prompt Guidance:**
For each subsystem, detail its purpose, inputs, and outputs relative to the full architecture.
Highlight how state, configuration, and shared utilities (e.g., auth, logging) propagate between them.
Ground descriptions in file paths, module names, and integration tests rather than abstractions.

**Evidence Checklist:**
- Primary structs, traits, and functions illustrating subsystem roles.
- Config keys, environment variables, or feature flags influencing behavior.
- Integration tests validating interactions among multiple subsystems.
"#;

const INTERFACE_SUMMARY_DEFAULT: &str = r#"# project_interface_summary: Global Interface & API Map

**Purpose:**
Describe all key interfaces—internal and external—that define the project’s contract surface, enabling safe integration across teams or services.

**Context:**
- **Surfaces:** Enumerate public crates, REST endpoints, RPC protocols, or CLI commands forming the project’s boundary.
- **Contracts:** Document data formats, trait definitions, and command semantics that external callers rely on.
- **Error & Versioning:** Capture project-wide patterns for error handling, retries, and compatibility evolution.
- **Instrumentation:** Note consistency standards for telemetry across interfaces.

**Prompt Guidance:**
List all significant interfaces, describing what each exposes and guarantees.
Define preconditions, outputs, side effects, and observable invariants.
Cite real call sites, schema files, and test coverage enforcing the contracts.
Show how interface style or conventions repeat across subsystems.

**Evidence Checklist:**
- Public APIs, schema definitions, or CLI specs.
- Mocks or contract tests guarding behavior.
- Example call paths verifying integration correctness.
"#;

const INVARIANT_CAPTURE_DEFAULT: &str = r#"# project_invariant_capture: Global Invariant Register

**Purpose:**
Collect the system-wide truths that must remain valid for the project to function correctly, explaining how each is enforced and monitored.

**Context:**
- **Motivation:** Clarify what system property would fail or corrupt state if the invariant were broken.
- **Scope:** Determine whether the invariant is local (per subsystem) or global (spanning multiple boundaries).
- **Enforcement:** Identify assertions, validations, or transaction boundaries maintaining the constraint.
- **Detection & Mitigation:** Reference metrics, alerts, and recovery flows that respond to violations.

**Prompt Guidance:**
Articulate each invariant in precise, testable form.
Tie it to concrete enforcement mechanisms in code or config.
Explain how the system detects and reacts to breaches.
Record known weaknesses or TODOs that threaten consistency.

**Evidence Checklist:**
- Assertions, guards, or schema constraints.
- Integration tests failing on violation.
- Operational runbooks or alert definitions linked to invariant health.
"#;

const OPERATIONAL_THREAD_DEFAULT: &str = r#"# project_operational_thread: System Operational Narratives

**Purpose:**
Describe major runtime or deployment flows across the project—how the system behaves when executed, deployed, or operated in production.

**Context:**
- **Triggers:** Identify initiating actions—user requests, cron tasks, event consumers, CI/CD triggers, or scheduled jobs.
- **Execution Paths:** Trace how these events traverse services, functions, queues, or threads.
- **Configuration:** List global and environment-specific settings influencing control flow.
- **Observability:** Summarize metrics, logs, and traces that reveal progress and failure.

**Prompt Guidance:**
Map the runtime sequences that represent key system stories (startup, deploy, background processing, user request).
Use concrete evidence: call stacks, async tasks, Docker compose definitions, or deployment manifests.
Call out branching logic, error-recovery strategies, and where operators can observe or intervene.

**Evidence Checklist:**
- Source files implementing each operational phase.
- Integration tests or scenario tests proving behavior.
- Logs, metrics, and tracing instrumentation used for runtime insight.
"#;
