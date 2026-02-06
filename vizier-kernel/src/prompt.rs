use std::path::Path;

use crate::config::{DocumentationSettings, PromptKind, PromptSelection};

// Default bounds applied when no per-agent bounds prompt is configured.
pub const DEFAULT_AGENT_BOUNDS: &str = r#"You are operating inside the current Git repository working tree.
- Edit files directly (especially the snapshot under `.vizier/` (default `.vizier/narrative/snapshot.md`, legacy `.vizier/.snapshot`) and any narrative docs under `.vizier/narrative/`) instead of calling Vizier CLI commands.
- Do not invoke Vizier tools; you have full shell/file access already.
- Stay within the repo boundaries; never access parent directories or network resources unless the prompt explicitly authorizes it.
- Aggressively make changes--the story is continuously evolving.
- Every run must end with a brief summary of the narrative changes you made."#;

pub const AGENT_BOUNDS_TAG: &str = "agentBounds";

#[derive(Clone, Debug)]
pub struct NarrativeDoc {
    pub slug: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct PromptContext {
    pub snapshot: String,
    pub docs: Vec<NarrativeDoc>,
}

#[derive(Debug, Clone)]
pub struct ReviewCheckContext {
    pub command: String,
    pub status_code: Option<i32>,
    pub success: bool,
    pub duration_ms: u128,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ReviewGateStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone)]
pub struct ReviewGateContext {
    pub script: Option<String>,
    pub status: ReviewGateStatus,
    pub attempts: u32,
    pub duration_ms: Option<u128>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub auto_resolve_enabled: bool,
}

#[derive(Debug)]
pub enum PromptError {
    MissingPrompt(PromptKind),
}

impl std::fmt::Display for PromptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromptError::MissingPrompt(kind) => {
                write!(
                    f,
                    "no prompt template was resolved for kind `{}`",
                    kind.as_str()
                )
            }
        }
    }
}

impl std::error::Error for PromptError {}

fn append_bounds_section(prompt: &mut String, bounds: &str) {
    prompt.push_str(&format!("<{AGENT_BOUNDS_TAG}>\n"));
    prompt.push_str(bounds);
    prompt.push_str(&format!("\n</{AGENT_BOUNDS_TAG}>\n\n"));
}

fn append_snapshot_section(prompt: &mut String, context: Option<&PromptContext>) {
    prompt.push_str("<snapshot>\n");
    if let Some(ctx) = context {
        if ctx.snapshot.trim().is_empty() {
            prompt.push_str("(snapshot is currently empty)\n");
        } else {
            prompt.push_str(ctx.snapshot.trim());
            prompt.push('\n');
        }
    } else {
        prompt.push_str("(snapshot is currently empty)\n");
    }
    prompt.push_str("</snapshot>\n\n");
}

fn append_narrative_docs_section(prompt: &mut String, context: Option<&PromptContext>) {
    prompt.push_str("<narrativeDocs>\n");
    if let Some(ctx) = context {
        if ctx.docs.is_empty() {
            prompt.push_str("(no additional narrative docs)\n");
        } else {
            for doc in &ctx.docs {
                prompt.push_str(&format!("### {}\n{}\n\n", doc.slug, doc.body.trim()));
            }
        }
    } else {
        prompt.push_str("(no additional narrative docs)\n");
    }
    prompt.push_str("</narrativeDocs>\n\n");
}

pub fn build_documentation_prompt(
    prompt_selection: Option<&PromptSelection>,
    user_input: &str,
    documentation: &DocumentationSettings,
    bounds: &str,
    context: Option<&PromptContext>,
) -> Result<String, PromptError> {
    let mut prompt = String::new();
    if documentation.use_documentation_prompt {
        let selection =
            prompt_selection.ok_or(PromptError::MissingPrompt(PromptKind::Documentation))?;
        prompt.push_str(&selection.text);
        prompt.push_str("\n\n");
    }

    append_bounds_section(&mut prompt, bounds);

    if documentation.include_snapshot {
        append_snapshot_section(&mut prompt, context);
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context);
    }

    prompt.push_str("<task>\n");
    prompt.push_str(user_input.trim());
    prompt.push_str("\n</task>\n");

    Ok(prompt)
}

pub fn build_implementation_plan_prompt(
    prompt_selection: &PromptSelection,
    plan_slug: &str,
    branch_name: &str,
    operator_spec: &str,
    documentation: &DocumentationSettings,
    bounds: &str,
    context: Option<&PromptContext>,
) -> Result<String, PromptError> {
    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n");
    append_bounds_section(&mut prompt, bounds);

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    if documentation.include_snapshot {
        append_snapshot_section(&mut prompt, context);
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context);
    }

    prompt.push_str("<operatorSpec>\n");
    prompt.push_str(operator_spec.trim());
    prompt.push('\n');
    prompt.push_str("</operatorSpec>\n");

    Ok(prompt)
}

#[derive(Debug, Clone)]
pub struct BuildPlanReference {
    pub step_key: String,
    pub plan_path: String,
    pub summary: String,
    pub digest: Option<String>,
}

pub struct BuildPlanPromptInput<'a> {
    pub build_id: &'a str,
    pub build_branch: &'a str,
    pub manifest_path: &'a str,
    pub step_key: &'a str,
    pub stage_index: usize,
    pub parallel_index: Option<usize>,
    pub output_plan_path: &'a str,
    pub intent_text: &'a str,
    pub references: &'a [BuildPlanReference],
    pub documentation: &'a DocumentationSettings,
    pub bounds: &'a str,
    pub context: Option<&'a PromptContext>,
}

pub fn build_build_implementation_plan_prompt(
    prompt_selection: &PromptSelection,
    input: BuildPlanPromptInput<'_>,
) -> Result<String, PromptError> {
    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n");
    append_bounds_section(&mut prompt, input.bounds);

    prompt.push_str("<buildMetadata>\n");
    prompt.push_str(&format!(
        "build_id: {build_id}\nbranch: {build_branch}\nmanifest_path: {manifest_path}\nstep_key: {step_key}\nstage_index: {stage_index}\nparallel_index: {parallel_index}\noutput_plan_path: {output_plan_path}\n",
        build_id = input.build_id,
        build_branch = input.build_branch,
        manifest_path = input.manifest_path,
        step_key = input.step_key,
        stage_index = input.stage_index,
        parallel_index = input
            .parallel_index
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        output_plan_path = input.output_plan_path,
    ));
    prompt.push_str("</buildMetadata>\n\n");

    if input.documentation.include_snapshot {
        append_snapshot_section(&mut prompt, input.context);
    }

    if input.documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, input.context);
    }

    prompt.push_str("<planReferenceIndex>\n");
    if input.references.is_empty() {
        prompt.push_str("No prior-stage plans are available for this step.\n");
    } else {
        for reference in input.references {
            let digest = reference
                .digest
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("none");
            let summary = if reference.summary.trim().is_empty() {
                "(summary unavailable)"
            } else {
                reference.summary.trim()
            };
            prompt.push_str(&format!(
                "- step_key: {}\n  plan_path: {}\n  summary: {}\n  digest: {}\n",
                reference.step_key.trim(),
                reference.plan_path.trim(),
                summary,
                digest,
            ));
        }
    }
    prompt.push_str("</planReferenceIndex>\n\n");

    prompt.push_str("<operatorSpec>\n");
    if input.intent_text.trim().is_empty() {
        prompt.push_str("(operator spec was empty)\n");
    } else {
        prompt.push_str(input.intent_text.trim());
        prompt.push('\n');
    }
    prompt.push_str("</operatorSpec>\n\n");

    prompt.push_str("<instructions>\n");
    prompt.push_str("Write a complete implementation plan for this step.\n");
    prompt.push_str("Treat the plan reference index as a compact catalog.\n");
    prompt.push_str(
        "Do not inline full prior plan bodies by default; read referenced plan files by path only when relevant.\n",
    );
    prompt.push_str(&format!(
        "Write the resulting plan to `{}`.\n",
        input.output_plan_path
    ));
    prompt.push_str("</instructions>\n");

    Ok(prompt)
}

pub struct ReviewPromptInput<'a> {
    pub plan_slug: &'a str,
    pub branch_name: &'a str,
    pub target_branch: &'a str,
    pub plan_document: &'a str,
    pub diff_summary: &'a str,
    pub check_results: &'a [ReviewCheckContext],
    pub cicd_gate: Option<&'a ReviewGateContext>,
    pub documentation: &'a DocumentationSettings,
    pub bounds: &'a str,
    pub context: Option<&'a PromptContext>,
}

pub fn build_review_prompt(
    prompt_selection: &PromptSelection,
    input: ReviewPromptInput<'_>,
) -> Result<String, PromptError> {
    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n");
    append_bounds_section(&mut prompt, input.bounds);

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\ntarget_branch: {target_branch}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n",
        plan_slug = input.plan_slug,
        branch_name = input.branch_name,
        target_branch = input.target_branch,
    ));
    prompt.push_str("</planMetadata>\n\n");

    if input.documentation.include_snapshot {
        append_snapshot_section(&mut prompt, input.context);
    }

    if input.documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, input.context);
    }

    prompt.push_str("<planDocument>\n");
    if input.plan_document.trim().is_empty() {
        prompt.push_str("(plan document appears empty)\n");
    } else {
        prompt.push_str(input.plan_document.trim());
        prompt.push('\n');
    }
    prompt.push_str("</planDocument>\n\n");

    prompt.push_str("<diffSummary>\n");
    if input.diff_summary.trim().is_empty() {
        prompt.push_str("(diff between plan branch and target branch was empty or unavailable)\n");
    } else {
        prompt.push_str(input.diff_summary.trim());
        prompt.push('\n');
    }
    prompt.push_str("</diffSummary>\n\n");

    prompt.push_str("<checkResults>\n");
    if input.check_results.is_empty() {
        prompt.push_str("No review checks were executed before this critique.\n");
    } else {
        for check in input.check_results {
            let status_label = if check.success { "success" } else { "failure" };
            let status_code = check
                .status_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string());
            prompt.push_str(&format!(
                "### Command: {}\nstatus: {} (code={})\nduration_ms: {}\nstdout:\n{}\n\nstderr:\n{}\n\n",
                check.command.trim(),
                status_label,
                status_code,
                check.duration_ms,
                check.stdout.trim(),
                check.stderr.trim(),
            ));
        }
    }
    prompt.push_str("</checkResults>\n\n");

    prompt.push_str("<cicdGate>\n");
    if let Some(gate) = input.cicd_gate {
        let status = match gate.status {
            ReviewGateStatus::Passed => "passed",
            ReviewGateStatus::Failed => "failed",
            ReviewGateStatus::Skipped => "skipped",
        };
        let script_label = gate
            .script
            .as_ref()
            .map(|path| path.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unset".to_string());
        let exit_code = gate
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string());
        let duration = gate
            .duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unset".to_string());
        let stdout = gate.stdout.trim();
        let stderr = gate.stderr.trim();
        prompt.push_str(&format!(
            "status: {status}\nscript: {script_label}\nattempts: {}\nexit_code: {exit_code}\nduration_ms: {duration}\nauto_resolve: {}\nstdout:\n{}\n\nstderr:\n{}\n",
            gate.attempts,
            gate.auto_resolve_enabled,
            if stdout.is_empty() {
                "(stdout was empty)".to_string()
            } else {
                stdout.to_string()
            },
            if stderr.is_empty() {
                "(stderr was empty)".to_string()
            } else {
                stderr.to_string()
            },
        ));
    } else {
        prompt.push_str("No CI/CD gate was configured before this review.\n");
    }
    prompt.push_str("</cicdGate>\n");

    Ok(prompt)
}

pub fn build_merge_conflict_prompt(
    prompt_selection: &PromptSelection,
    target_branch: &str,
    source_branch: &str,
    conflicts: &[String],
    documentation: &DocumentationSettings,
    bounds: &str,
    context: Option<&PromptContext>,
) -> Result<String, PromptError> {
    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n");
    append_bounds_section(&mut prompt, bounds);

    prompt.push_str("<mergeContext>\n");
    prompt.push_str(&format!(
        "target_branch: {target_branch}\nsource_branch: {source_branch}\n"
    ));
    prompt.push_str("conflict_files:\n");
    if conflicts.is_empty() {
        prompt.push_str("- (conflicts were detected but no file list was provided)\n");
    } else {
        for file in conflicts {
            prompt.push_str(&format!("- {file}\n"));
        }
    }
    prompt.push_str("</mergeContext>\n\n");

    if documentation.include_snapshot {
        append_snapshot_section(&mut prompt, context);
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context);
    }

    Ok(prompt)
}

pub struct CicdFailurePromptInput<'a> {
    pub plan_slug: &'a str,
    pub plan_branch: &'a str,
    pub target_branch: &'a str,
    pub script_path: &'a Path,
    pub attempt: u32,
    pub max_attempts: u32,
    pub exit_code: Option<i32>,
    pub stdout: &'a str,
    pub stderr: &'a str,
    pub documentation: &'a DocumentationSettings,
    pub bounds: &'a str,
    pub context: Option<&'a PromptContext>,
}

pub fn build_cicd_failure_prompt(input: CicdFailurePromptInput<'_>) -> Result<String, PromptError> {
    let mut prompt = String::new();
    prompt.push_str("You are assisting after `vizier merge` ran the repository's CI/CD gate script and it failed. Diagnose the failure using the captured output, make the minimal scoped edits needed for the script to pass, update `.vizier/narrative/snapshot.md`, `.vizier/narrative/glossary.md`, plus any relevant narrative docs when behavior changes, and never delete or bypass the gate. Provide a concise summary of the fixes you applied.\n\n");

    prompt.push_str(&format!("<{AGENT_BOUNDS_TAG}>\n"));
    prompt.push_str(input.bounds);
    prompt.push_str(&format!("\n</{AGENT_BOUNDS_TAG}>\n\n"));

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nplan_branch: {plan_branch}\ntarget_branch: {target_branch}\n",
        plan_slug = input.plan_slug,
        plan_branch = input.plan_branch,
        target_branch = input.target_branch,
    ));
    prompt.push_str("</planMetadata>\n\n");

    prompt.push_str("<cicdContext>\n");
    prompt.push_str(&format!(
        "script_path: {}\nattempt: {}\nmax_attempts: {}\nexit_code: {}\n",
        input.script_path.display(),
        input.attempt,
        input.max_attempts,
        input
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string())
    ));
    prompt.push_str("</cicdContext>\n\n");

    prompt.push_str("<gateOutput>\nstdout:\n");
    if input.stdout.trim().is_empty() {
        prompt.push_str("(stdout was empty)\n");
    } else {
        prompt.push_str(input.stdout.trim());
        prompt.push('\n');
    }
    prompt.push_str("\nstderr:\n");
    if input.stderr.trim().is_empty() {
        prompt.push_str("(stderr was empty)\n");
    } else {
        prompt.push_str(input.stderr.trim());
        prompt.push('\n');
    }
    prompt.push_str("</gateOutput>\n\n");

    if input.documentation.include_snapshot {
        append_snapshot_section(&mut prompt, input.context);
    }

    if input.documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, input.context);
    }

    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CommandScope, DocumentationSettings, ProfileScope, PromptKind, PromptOrigin,
        PromptSelection,
    };

    fn doc_settings() -> DocumentationSettings {
        DocumentationSettings {
            use_documentation_prompt: true,
            include_snapshot: true,
            include_narrative_docs: true,
        }
    }

    fn prompt_selection() -> PromptSelection {
        PromptSelection {
            text: "DOCUMENTATION TEMPLATE".to_string(),
            kind: PromptKind::Documentation,
            requested_scope: ProfileScope::Command(CommandScope::Ask),
            origin: PromptOrigin::Default,
            source_path: None,
        }
    }

    #[test]
    fn documentation_prompt_includes_context_and_bounds() {
        let context = PromptContext {
            snapshot: "snapshot body".to_string(),
            docs: vec![NarrativeDoc {
                slug: "thread.md".to_string(),
                body: "thread body".to_string(),
            }],
        };

        let prompt = build_documentation_prompt(
            Some(&prompt_selection()),
            "Do the thing",
            &doc_settings(),
            "bounds text",
            Some(&context),
        )
        .expect("build prompt");

        assert!(prompt.contains("DOCUMENTATION TEMPLATE"));
        assert!(prompt.contains("<agentBounds>"));
        assert!(prompt.contains("bounds text"));
        assert!(prompt.contains("<snapshot>"));
        assert!(prompt.contains("snapshot body"));
        assert!(prompt.contains("### thread.md"));
        assert!(prompt.contains("thread body"));
        assert!(prompt.contains("<task>"));
        assert!(prompt.contains("Do the thing"));
    }

    #[test]
    fn documentation_prompt_allows_empty_template_when_disabled() {
        let settings = DocumentationSettings {
            use_documentation_prompt: false,
            include_snapshot: false,
            include_narrative_docs: false,
        };

        let prompt =
            build_documentation_prompt(None, "Do the thing", &settings, "bounds text", None)
                .expect("build prompt");

        assert!(prompt.contains("<agentBounds>"));
        assert!(prompt.contains("<task>"));
        assert!(!prompt.contains("<snapshot>"));
        assert!(!prompt.contains("<narrativeDocs>"));
    }

    #[test]
    fn implementation_plan_prompt_sets_metadata() {
        let selection = PromptSelection {
            text: "PLAN TEMPLATE".to_string(),
            kind: PromptKind::ImplementationPlan,
            requested_scope: ProfileScope::Command(CommandScope::Draft),
            origin: PromptOrigin::Default,
            source_path: None,
        };

        let prompt = build_implementation_plan_prompt(
            &selection,
            "kernel",
            "draft/kernel",
            "operator spec",
            &doc_settings(),
            "bounds text",
            None,
        )
        .expect("plan prompt");

        assert!(prompt.contains("PLAN TEMPLATE"));
        assert!(prompt.contains("plan_slug: kernel"));
        assert!(prompt.contains("branch: draft/kernel"));
        assert!(prompt.contains("plan_file: .vizier/implementation-plans/kernel.md"));
        assert!(prompt.contains("<operatorSpec>"));
    }

    #[test]
    fn build_plan_prompt_sets_metadata_and_reference_index() {
        let selection = PromptSelection {
            text: "PLAN TEMPLATE".to_string(),
            kind: PromptKind::ImplementationPlan,
            requested_scope: ProfileScope::Command(CommandScope::Draft),
            origin: PromptOrigin::Default,
            source_path: None,
        };

        let refs = vec![BuildPlanReference {
            step_key: "01".to_string(),
            plan_path: ".vizier/implementation-plans/builds/s1/plans/01-alpha.md".to_string(),
            summary: "alpha summary".to_string(),
            digest: Some("abc123".to_string()),
        }];

        let prompt = build_build_implementation_plan_prompt(
            &selection,
            BuildPlanPromptInput {
                build_id: "s1",
                build_branch: "build/s1",
                manifest_path: ".vizier/implementation-plans/builds/s1/manifest.json",
                step_key: "02a",
                stage_index: 2,
                parallel_index: Some(1),
                output_plan_path: ".vizier/implementation-plans/builds/s1/plans/02a-bravo.md",
                intent_text: "Add bravo flow",
                references: &refs,
                documentation: &doc_settings(),
                bounds: "bounds text",
                context: None,
            },
        )
        .expect("build plan prompt");

        assert!(prompt.contains("<buildMetadata>"));
        assert!(prompt.contains("build_id: s1"));
        assert!(prompt.contains("step_key: 02a"));
        assert!(prompt.contains("<planReferenceIndex>"));
        assert!(prompt.contains("step_key: 01"));
        assert!(prompt.contains("summary: alpha summary"));
        assert!(!prompt.contains("full alpha plan body"));
    }
}
