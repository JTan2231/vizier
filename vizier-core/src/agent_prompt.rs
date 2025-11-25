use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{agent::AgentError, config, tools};

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

pub fn gather_prompt_context() -> Result<PromptContext, AgentError> {
    let narrative_dir = tools::try_get_narrative_dir();

    let snapshot = load_snapshot();
    let docs = read_narrative_docs(narrative_dir.as_deref())?;

    Ok(PromptContext { snapshot, docs })
}

fn load_snapshot() -> String {
    if let Some(path) = tools::try_snapshot_path() {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            return contents;
        }
    }

    String::new()
}

fn read_narrative_docs(narrative_dir: Option<&str>) -> Result<Vec<NarrativeDoc>, AgentError> {
    let Some(dir) = narrative_dir else {
        return Ok(Vec::new());
    };

    let root = PathBuf::from(dir);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut docs = Vec::new();
    let mut stack = vec![root.clone()];

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            if path
                .file_name()
                .and_then(OsStr::to_str)
                .map(|name| name == tools::SNAPSHOT_FILE)
                .unwrap_or(false)
            {
                continue;
            }

            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                continue;
            }

            let body = std::fs::read_to_string(&path).unwrap_or_default();
            let slug = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");

            docs.push(NarrativeDoc { slug, body });
        }
    }

    docs.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(docs)
}

fn load_context_if_needed(
    include_snapshot: bool,
    include_narrative_docs: bool,
) -> Result<Option<PromptContext>, AgentError> {
    if !include_snapshot && !include_narrative_docs {
        return Ok(None);
    }

    gather_prompt_context().map(Some)
}

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
    prompt_selection: Option<&config::PromptSelection>,
    user_input: &str,
    documentation: &config::DocumentationSettings,
) -> Result<String, AgentError> {
    let context = load_context_if_needed(
        documentation.include_snapshot,
        documentation.include_narrative_docs,
    )?;
    let bounds = load_bounds_prompt()?;

    let mut prompt = String::new();
    if documentation.use_documentation_prompt {
        let selection = prompt_selection
            .ok_or_else(|| AgentError::MissingPrompt(config::PromptKind::Documentation))?;
        prompt.push_str(&selection.text);
        prompt.push_str("\n\n");
    }

    append_bounds_section(&mut prompt, &bounds);

    if documentation.include_snapshot {
        append_snapshot_section(&mut prompt, context.as_ref());
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context.as_ref());
    }

    prompt.push_str("<task>\n");
    prompt.push_str(user_input.trim());
    prompt.push_str("\n</task>\n");

    Ok(prompt)
}

pub fn build_implementation_plan_prompt(
    prompt_selection: &config::PromptSelection,
    plan_slug: &str,
    branch_name: &str,
    operator_spec: &str,
    documentation: &config::DocumentationSettings,
) -> Result<String, AgentError> {
    let context = load_context_if_needed(
        documentation.include_snapshot,
        documentation.include_narrative_docs,
    )?;
    let bounds = load_bounds_prompt()?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n");
    append_bounds_section(&mut prompt, &bounds);

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    if documentation.include_snapshot {
        append_snapshot_section(&mut prompt, context.as_ref());
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context.as_ref());
    }

    prompt.push_str("<operatorSpec>\n");
    prompt.push_str(operator_spec.trim());
    prompt.push('\n');
    prompt.push_str("</operatorSpec>\n");

    Ok(prompt)
}

pub fn build_review_prompt(
    prompt_selection: &config::PromptSelection,
    plan_slug: &str,
    branch_name: &str,
    target_branch: &str,
    plan_document: &str,
    diff_summary: &str,
    check_results: &[crate::agent::ReviewCheckContext],
    cicd_gate: Option<&crate::agent::ReviewGateContext>,
    documentation: &config::DocumentationSettings,
) -> Result<String, AgentError> {
    let context = load_context_if_needed(
        documentation.include_snapshot,
        documentation.include_narrative_docs,
    )?;
    let bounds = load_bounds_prompt()?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n");
    append_bounds_section(&mut prompt, &bounds);

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\ntarget_branch: {target_branch}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    if documentation.include_snapshot {
        append_snapshot_section(&mut prompt, context.as_ref());
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context.as_ref());
    }

    prompt.push_str("<planDocument>\n");
    if plan_document.trim().is_empty() {
        prompt.push_str("(plan document appears empty)\n");
    } else {
        prompt.push_str(plan_document.trim());
        prompt.push('\n');
    }
    prompt.push_str("</planDocument>\n\n");

    prompt.push_str("<diffSummary>\n");
    if diff_summary.trim().is_empty() {
        prompt.push_str("(diff between plan branch and target branch was empty or unavailable)\n");
    } else {
        prompt.push_str(diff_summary.trim());
        prompt.push('\n');
    }
    prompt.push_str("</diffSummary>\n\n");

    prompt.push_str("<checkResults>\n");
    if check_results.is_empty() {
        prompt.push_str("No review checks were executed before this critique.\n");
    } else {
        for check in check_results {
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
    if let Some(gate) = cicd_gate {
        let status = match gate.status {
            crate::agent::ReviewGateStatus::Passed => "passed",
            crate::agent::ReviewGateStatus::Failed => "failed",
            crate::agent::ReviewGateStatus::Skipped => "skipped",
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
    prompt_selection: &config::PromptSelection,
    target_branch: &str,
    source_branch: &str,
    conflicts: &[String],
    documentation: &config::DocumentationSettings,
) -> Result<String, AgentError> {
    let context = load_context_if_needed(
        documentation.include_snapshot,
        documentation.include_narrative_docs,
    )?;
    let bounds = load_bounds_prompt()?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str("\n\n");
    append_bounds_section(&mut prompt, &bounds);

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
        append_snapshot_section(&mut prompt, context.as_ref());
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context.as_ref());
    }

    Ok(prompt)
}

pub fn build_cicd_failure_prompt(
    plan_slug: &str,
    plan_branch: &str,
    target_branch: &str,
    script_path: &Path,
    attempt: u32,
    max_attempts: u32,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    documentation: &config::DocumentationSettings,
) -> Result<String, AgentError> {
    let context = load_context_if_needed(
        documentation.include_snapshot,
        documentation.include_narrative_docs,
    )?;
    let bounds = load_bounds_prompt()?;

    let mut prompt = String::new();
    prompt.push_str("You are assisting after `vizier merge` ran the repository's CI/CD gate script and it failed. Diagnose the failure using the captured output, make the minimal scoped edits needed for the script to pass, update `.vizier/narrative/snapshot.md` plus any relevant narrative docs when behavior changes, and never delete or bypass the gate. Provide a concise summary of the fixes you applied.\n\n");

    prompt.push_str(&format!("<{AGENT_BOUNDS_TAG}>\n"));
    prompt.push_str(&bounds);
    prompt.push_str(&format!("\n</{AGENT_BOUNDS_TAG}>\n\n"));

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nplan_branch: {plan_branch}\ntarget_branch: {target_branch}\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    prompt.push_str("<cicdContext>\n");
    prompt.push_str(&format!(
        "script_path: {}\nattempt: {}\nmax_attempts: {}\nexit_code: {}\n",
        script_path.display(),
        attempt,
        max_attempts,
        exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string())
    ));
    prompt.push_str("</cicdContext>\n\n");

    prompt.push_str("<gateOutput>\nstdout:\n");
    if stdout.trim().is_empty() {
        prompt.push_str("(stdout was empty)\n");
    } else {
        prompt.push_str(stdout.trim());
        prompt.push('\n');
    }
    prompt.push_str("\nstderr:\n");
    if stderr.trim().is_empty() {
        prompt.push_str("(stderr was empty)\n");
    } else {
        prompt.push_str(stderr.trim());
        prompt.push('\n');
    }
    prompt.push_str("</gateOutput>\n\n");

    if documentation.include_snapshot {
        append_snapshot_section(&mut prompt, context.as_ref());
    }

    if documentation.include_narrative_docs {
        append_narrative_docs_section(&mut prompt, context.as_ref());
    }

    Ok(prompt)
}

fn load_bounds_prompt() -> Result<String, AgentError> {
    Ok(DEFAULT_AGENT_BOUNDS.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, CommandScope, DocumentationSettings, PromptKind};
    use std::sync::Mutex;

    static CONFIG_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn implementation_plan_prompt_respects_override() {
        let _guard = CONFIG_LOCK.lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.set_prompt(PromptKind::ImplementationPlan, "custom plan".to_string());
        config::set_config(cfg);

        let selection =
            config::get_config().prompt_for(CommandScope::Draft, PromptKind::ImplementationPlan);
        let prompt = build_implementation_plan_prompt(
            &selection,
            "slug",
            "draft/slug",
            "spec",
            &DocumentationSettings::default(),
        )
        .unwrap();

        assert!(prompt.starts_with("custom plan"));
        assert!(prompt.contains("<agentBounds>"));

        config::set_config(original);
    }

    #[test]
    fn review_prompt_respects_override() {
        let _guard = CONFIG_LOCK.lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.set_prompt(PromptKind::Review, "custom review".to_string());
        config::set_config(cfg);

        let selection = config::get_config().prompt_for(CommandScope::Review, PromptKind::Review);
        let prompt = build_review_prompt(
            &selection,
            "slug",
            "draft/slug",
            "main",
            "plan",
            "diff",
            &[],
            None,
            &DocumentationSettings::default(),
        )
        .unwrap();

        assert!(prompt.starts_with("custom review"));
        assert!(prompt.contains("<planDocument>"));

        config::set_config(original);
    }

    #[test]
    fn review_prompt_includes_cicd_gate_context() {
        let _guard = CONFIG_LOCK.lock().unwrap();
        let original = config::get_config();
        let selection = config::get_config().prompt_for(CommandScope::Review, PromptKind::Review);
        let gate = crate::agent::ReviewGateContext {
            script: Some("cicd.sh".to_string()),
            status: crate::agent::ReviewGateStatus::Failed,
            attempts: 1,
            duration_ms: Some(1200),
            exit_code: Some(2),
            stdout: "gate stdout".to_string(),
            stderr: String::new(),
            auto_resolve_enabled: false,
        };
        let prompt = build_review_prompt(
            &selection,
            "slug",
            "draft/slug",
            "main",
            "plan",
            "diff",
            &[],
            Some(&gate),
            &DocumentationSettings::default(),
        )
        .unwrap();

        assert!(prompt.contains("<cicdGate>"));
        assert!(prompt.contains("status: failed"));
        assert!(prompt.contains("script: cicd.sh"));
        assert!(prompt.contains("exit_code: 2"));
        assert!(prompt.contains("stdout:\ngate stdout"));

        config::set_config(original);
    }

    #[test]
    fn merge_conflict_prompt_respects_override() {
        let _guard = CONFIG_LOCK.lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.set_prompt(PromptKind::MergeConflict, "custom merge".to_string());
        config::set_config(cfg);

        let conflicts = vec!["src/lib.rs".to_string()];
        let selection =
            config::get_config().prompt_for(CommandScope::Merge, PromptKind::MergeConflict);
        let prompt = build_merge_conflict_prompt(
            &selection,
            "main",
            "draft/slug",
            &conflicts,
            &DocumentationSettings::default(),
        )
        .unwrap();

        assert!(prompt.starts_with("custom merge"));
        assert!(prompt.contains("<mergeContext>"));

        config::set_config(original);
    }

    #[test]
    fn documentation_prompt_can_skip_snapshot_and_threads() {
        let settings = DocumentationSettings {
            use_documentation_prompt: true,
            include_snapshot: false,
            include_narrative_docs: false,
        };
        let selection =
            config::get_config().prompt_for(CommandScope::Ask, PromptKind::Documentation);
        let prompt = build_documentation_prompt(Some(&selection), "do the thing", &settings)
            .expect("prompt builds");

        assert!(prompt.contains("<mainInstruction>"));
        assert!(prompt.contains("<task>\ndo the thing\n</task>"));
        assert!(!prompt.contains("<snapshot>\n"));
        assert!(!prompt.contains("<narrativeDocs>\n"));
    }

    #[test]
    fn documentation_prompt_respects_disabled_template() {
        let settings = DocumentationSettings {
            use_documentation_prompt: false,
            include_snapshot: false,
            include_narrative_docs: false,
        };

        let prompt =
            build_documentation_prompt(None, "just do it", &settings).expect("build prompt");

        assert!(!prompt.contains("<mainInstruction>"));
        assert!(prompt.contains("<agentBounds>"));
        assert!(prompt.contains("<task>\njust do it\n</task>"));
    }
}
