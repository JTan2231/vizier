use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{agent::AgentError, config, tools};

// Default bounds applied when no per-agent bounds prompt is configured.
pub const DEFAULT_AGENT_BOUNDS: &str = r#"You are operating inside the current Git repository working tree.
- Edit files directly (especially `.vizier/.snapshot` and TODO artifacts) instead of calling Vizier CLI commands.
- Do not invoke Vizier tools; you have full shell/file access already.
- Stay within the repo boundaries; never access parent directories or network resources unless the prompt explicitly authorizes it.
- Aggressively make changes--the story is continuously evolving.
- Every run must end with a brief summary of the narrative changes you made."#;

pub const AGENT_BOUNDS_TAG: &str = "agentBounds";

#[derive(Clone, Debug)]
pub struct ThreadArtifact {
    pub slug: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct PromptContext {
    pub snapshot: String,
    pub threads: Vec<ThreadArtifact>,
}

pub fn gather_prompt_context() -> Result<PromptContext, AgentError> {
    let snapshot_path = PathBuf::from(format!("{}{}", tools::get_todo_dir(), ".snapshot"));
    let snapshot = std::fs::read_to_string(&snapshot_path).unwrap_or_default();
    let threads = read_thread_files(&PathBuf::from(tools::get_todo_dir()))?;

    Ok(PromptContext { snapshot, threads })
}

fn read_thread_files(dir: &Path) -> Result<Vec<ThreadArtifact>, AgentError> {
    let mut threads = Vec::new();

    if !dir.exists() {
        return Ok(threads);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if path
            .file_name()
            .and_then(OsStr::to_str)
            .map(|name| name == ".snapshot")
            .unwrap_or(false)
        {
            continue;
        }

        let slug = match path.file_name().and_then(OsStr::to_str) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let body = std::fs::read_to_string(&path).unwrap_or_default();
        threads.push(ThreadArtifact { slug, body });
    }

    threads.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(threads)
}

fn build_prompt(
    prompt_selection: &config::PromptSelection,
    snapshot: &str,
    threads: &[ThreadArtifact],
    user_input: &str,
    bounds_override: Option<&Path>,
) -> Result<String, AgentError> {
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str(&format!("\n\n<{AGENT_BOUNDS_TAG}>\n"));
    prompt.push_str(&bounds);
    prompt.push_str(&format!("\n</{AGENT_BOUNDS_TAG}>\n\n"));

    prompt.push_str("<snapshot>\n");
    if snapshot.trim().is_empty() {
        prompt.push_str("(snapshot is currently empty)\n");
    } else {
        prompt.push_str(snapshot.trim());
        prompt.push('\n');
    }
    prompt.push_str("</snapshot>\n\n");

    prompt.push_str("<todoThreads>\n");
    if threads.is_empty() {
        prompt.push_str("(no active TODO threads)\n");
    } else {
        for thread in threads {
            prompt.push_str(&format!("### {}\n{}\n\n", thread.slug, thread.body.trim()));
        }
    }
    prompt.push_str("</todoThreads>\n\n");

    prompt.push_str("<task>\n");
    prompt.push_str(user_input.trim());
    prompt.push_str("\n</task>\n");

    Ok(prompt)
}

pub fn build_base_prompt(
    prompt_selection: &config::PromptSelection,
    user_input: &str,
    bounds_override: Option<&Path>,
) -> Result<String, AgentError> {
    let context = gather_prompt_context()?;
    build_prompt(
        prompt_selection,
        &context.snapshot,
        &context.threads,
        user_input,
        bounds_override,
    )
}

pub fn build_implementation_plan_prompt(
    prompt_selection: &config::PromptSelection,
    plan_slug: &str,
    branch_name: &str,
    operator_spec: &str,
    bounds_override: Option<&Path>,
) -> Result<String, AgentError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str(&format!("\n\n<{AGENT_BOUNDS_TAG}>\n"));
    prompt.push_str(&bounds);
    prompt.push_str(&format!("\n</{AGENT_BOUNDS_TAG}>\n\n"));

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    prompt.push_str("<snapshot>\n");
    if context.snapshot.trim().is_empty() {
        prompt.push_str("(snapshot is currently empty)\n");
    } else {
        prompt.push_str(context.snapshot.trim());
        prompt.push('\n');
    }
    prompt.push_str("</snapshot>\n\n");

    prompt.push_str("<todoThreads>\n");
    if context.threads.is_empty() {
        prompt.push_str("(no active TODO threads)\n");
    } else {
        for thread in &context.threads {
            prompt.push_str(&format!("### {}\n{}\n\n", thread.slug, thread.body.trim()));
        }
    }
    prompt.push_str("</todoThreads>\n\n");

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
    bounds_override: Option<&Path>,
) -> Result<String, AgentError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str(&format!("\n\n<{AGENT_BOUNDS_TAG}>\n"));
    prompt.push_str(&bounds);
    prompt.push_str(&format!("\n</{AGENT_BOUNDS_TAG}>\n\n"));

    prompt.push_str("<planMetadata>\n");
    prompt.push_str(&format!(
        "plan_slug: {plan_slug}\nbranch: {branch_name}\ntarget_branch: {target_branch}\nplan_file: .vizier/implementation-plans/{plan_slug}.md\n"
    ));
    prompt.push_str("</planMetadata>\n\n");

    prompt.push_str("<snapshot>\n");
    if context.snapshot.trim().is_empty() {
        prompt.push_str("(snapshot is currently empty)\n");
    } else {
        prompt.push_str(context.snapshot.trim());
        prompt.push('\n');
    }
    prompt.push_str("</snapshot>\n\n");

    prompt.push_str("<todoThreads>\n");
    if context.threads.is_empty() {
        prompt.push_str("(no active TODO threads)\n");
    } else {
        for thread in &context.threads {
            prompt.push_str(&format!("### {}\n{}\n\n", thread.slug, thread.body.trim()));
        }
    }
    prompt.push_str("</todoThreads>\n\n");

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
    prompt.push_str("</checkResults>\n");

    Ok(prompt)
}

pub fn build_merge_conflict_prompt(
    prompt_selection: &config::PromptSelection,
    target_branch: &str,
    source_branch: &str,
    conflicts: &[String],
    bounds_override: Option<&Path>,
) -> Result<String, AgentError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str(&prompt_selection.text);
    prompt.push_str(&format!("\n\n<{AGENT_BOUNDS_TAG}>\n"));
    prompt.push_str(&bounds);
    prompt.push_str(&format!("\n</{AGENT_BOUNDS_TAG}>\n\n"));

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

    prompt.push_str("<snapshot>\n");
    if context.snapshot.trim().is_empty() {
        prompt.push_str("(snapshot is currently empty)\n");
    } else {
        prompt.push_str(context.snapshot.trim());
        prompt.push('\n');
    }
    prompt.push_str("</snapshot>\n\n");

    prompt.push_str("<todoThreads>\n");
    if context.threads.is_empty() {
        prompt.push_str("(no active TODO threads)\n");
    } else {
        for thread in &context.threads {
            prompt.push_str(&format!("### {}\n{}\n\n", thread.slug, thread.body.trim()));
        }
    }
    prompt.push_str("</todoThreads>\n");

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
    bounds_override: Option<&Path>,
) -> Result<String, AgentError> {
    let context = gather_prompt_context()?;
    let bounds = load_bounds_prompt(bounds_override)?;

    let mut prompt = String::new();
    prompt.push_str("You are assisting after `vizier merge` ran the repository's CI/CD gate script and it failed. Diagnose the failure using the captured output, make the minimal scoped edits needed for the script to pass, update `.vizier/.snapshot` plus TODO threads when behavior changes, and never delete or bypass the gate. Provide a concise summary of the fixes you applied.\n\n");

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

    prompt.push_str("<snapshot>\n");
    if context.snapshot.trim().is_empty() {
        prompt.push_str("(snapshot is currently empty)\n");
    } else {
        prompt.push_str(context.snapshot.trim());
        prompt.push('\n');
    }
    prompt.push_str("</snapshot>\n\n");

    prompt.push_str("<todoThreads>\n");
    if context.threads.is_empty() {
        prompt.push_str("(no active TODO threads)\n");
    } else {
        for thread in &context.threads {
            prompt.push_str(&format!("### {}\n{}\n\n", thread.slug, thread.body.trim()));
        }
    }
    prompt.push_str("</todoThreads>\n\n");

    Ok(prompt)
}

fn load_bounds_prompt(bounds_override: Option<&Path>) -> Result<String, AgentError> {
    if let Some(path) = bounds_override {
        let contents = std::fs::read_to_string(path)
            .map_err(|err| AgentError::BoundsRead(path.to_path_buf(), err))?;
        return Ok(contents);
    }

    if let Some(path) = &config::get_config().agent_runtime.bounds_prompt_path {
        let contents = std::fs::read_to_string(path)
            .map_err(|err| AgentError::BoundsRead(path.clone(), err))?;
        Ok(contents)
    } else {
        Ok(DEFAULT_AGENT_BOUNDS.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, CommandScope, PromptKind};
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
        let prompt =
            build_implementation_plan_prompt(&selection, "slug", "draft/slug", "spec", None)
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
        )
        .unwrap();

        assert!(prompt.starts_with("custom review"));
        assert!(prompt.contains("<planDocument>"));

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
        let prompt =
            build_merge_conflict_prompt(&selection, "main", "draft/slug", &conflicts, None)
                .unwrap();

        assert!(prompt.starts_with("custom merge"));
        assert!(prompt.contains("<mergeContext>"));

        config::set_config(original);
    }
}
