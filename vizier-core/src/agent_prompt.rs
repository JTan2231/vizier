use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{agent::AgentError, config, tools};
use vizier_kernel::prompt::{self as kernel_prompt, NarrativeDoc, PromptContext};

pub fn gather_prompt_context() -> Result<PromptContext, AgentError> {
    let narrative_dir = tools::try_get_narrative_dir();

    let snapshot = load_snapshot();
    let docs = read_narrative_docs(narrative_dir.as_deref())?;

    Ok(PromptContext { snapshot, docs })
}

fn load_snapshot() -> String {
    if let Some(path) = tools::try_snapshot_path()
        && let Ok(contents) = std::fs::read_to_string(&path)
    {
        return contents;
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

    docs.sort_by(|a, b| {
        let a_priority = if a.slug == tools::GLOSSARY_FILE { 0 } else { 1 };
        let b_priority = if b.slug == tools::GLOSSARY_FILE { 0 } else { 1 };
        a_priority
            .cmp(&b_priority)
            .then_with(|| a.slug.cmp(&b.slug))
    });
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

fn map_prompt_error(error: kernel_prompt::PromptError) -> AgentError {
    match error {
        kernel_prompt::PromptError::MissingPrompt(kind) => AgentError::MissingPrompt(kind),
    }
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
    kernel_prompt::build_documentation_prompt(
        prompt_selection,
        user_input,
        documentation,
        &bounds,
        context.as_ref(),
    )
    .map_err(map_prompt_error)
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
    kernel_prompt::build_implementation_plan_prompt(
        prompt_selection,
        plan_slug,
        branch_name,
        operator_spec,
        documentation,
        &bounds,
        context.as_ref(),
    )
    .map_err(map_prompt_error)
}

pub struct ReviewPromptInput<'a> {
    pub plan_slug: &'a str,
    pub branch_name: &'a str,
    pub target_branch: &'a str,
    pub plan_document: &'a str,
    pub diff_summary: &'a str,
    pub check_results: &'a [crate::agent::ReviewCheckContext],
    pub cicd_gate: Option<&'a crate::agent::ReviewGateContext>,
    pub documentation: &'a config::DocumentationSettings,
}

pub fn build_review_prompt(
    prompt_selection: &config::PromptSelection,
    input: ReviewPromptInput<'_>,
) -> Result<String, AgentError> {
    let context = load_context_if_needed(
        input.documentation.include_snapshot,
        input.documentation.include_narrative_docs,
    )?;
    let bounds = load_bounds_prompt()?;

    let kernel_input = kernel_prompt::ReviewPromptInput {
        plan_slug: input.plan_slug,
        branch_name: input.branch_name,
        target_branch: input.target_branch,
        plan_document: input.plan_document,
        diff_summary: input.diff_summary,
        check_results: input.check_results,
        cicd_gate: input.cicd_gate,
        documentation: input.documentation,
        bounds: &bounds,
        context: context.as_ref(),
    };

    kernel_prompt::build_review_prompt(prompt_selection, kernel_input).map_err(map_prompt_error)
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
    kernel_prompt::build_merge_conflict_prompt(
        prompt_selection,
        target_branch,
        source_branch,
        conflicts,
        documentation,
        &bounds,
        context.as_ref(),
    )
    .map_err(map_prompt_error)
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
    pub documentation: &'a config::DocumentationSettings,
}

pub fn build_cicd_failure_prompt(input: CicdFailurePromptInput<'_>) -> Result<String, AgentError> {
    let context = load_context_if_needed(
        input.documentation.include_snapshot,
        input.documentation.include_narrative_docs,
    )?;
    let bounds = load_bounds_prompt()?;
    let kernel_input = kernel_prompt::CicdFailurePromptInput {
        plan_slug: input.plan_slug,
        plan_branch: input.plan_branch,
        target_branch: input.target_branch,
        script_path: input.script_path,
        attempt: input.attempt,
        max_attempts: input.max_attempts,
        exit_code: input.exit_code,
        stdout: input.stdout,
        stderr: input.stderr,
        documentation: input.documentation,
        bounds: &bounds,
        context: context.as_ref(),
    };

    kernel_prompt::build_cicd_failure_prompt(kernel_input).map_err(map_prompt_error)
}

fn load_bounds_prompt() -> Result<String, AgentError> {
    Ok(kernel_prompt::DEFAULT_AGENT_BOUNDS.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, CommandScope, DocumentationSettings, PromptKind, PromptOverrides};

    #[test]
    fn implementation_plan_prompt_respects_override() {
        let _guard = config::test_config_lock().lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.agent_defaults.prompt_overrides.insert(
            PromptKind::ImplementationPlan,
            PromptOverrides {
                text: Some("custom plan".to_string()),
                source_path: None,
                agent: None,
            },
        );
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
        let _guard = config::test_config_lock().lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.agent_defaults.prompt_overrides.insert(
            PromptKind::Review,
            PromptOverrides {
                text: Some("custom review".to_string()),
                source_path: None,
                agent: None,
            },
        );
        config::set_config(cfg);

        let selection = config::get_config().prompt_for(CommandScope::Review, PromptKind::Review);
        let prompt = build_review_prompt(
            &selection,
            ReviewPromptInput {
                plan_slug: "slug",
                branch_name: "draft/slug",
                target_branch: "main",
                plan_document: "plan",
                diff_summary: "diff",
                check_results: &[],
                cicd_gate: None,
                documentation: &DocumentationSettings::default(),
            },
        )
        .unwrap();

        assert!(prompt.starts_with("custom review"));
        assert!(prompt.contains("<planDocument>"));

        config::set_config(original);
    }

    #[test]
    fn review_prompt_includes_cicd_gate_context() {
        let _guard = config::test_config_lock().lock().unwrap();
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
            ReviewPromptInput {
                plan_slug: "slug",
                branch_name: "draft/slug",
                target_branch: "main",
                plan_document: "plan",
                diff_summary: "diff",
                check_results: &[],
                cicd_gate: Some(&gate),
                documentation: &DocumentationSettings::default(),
            },
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
        let _guard = config::test_config_lock().lock().unwrap();
        let original = config::get_config();
        let mut cfg = original.clone();
        cfg.agent_defaults.prompt_overrides.insert(
            PromptKind::MergeConflict,
            PromptOverrides {
                text: Some("custom merge".to_string()),
                source_path: None,
                agent: None,
            },
        );
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
