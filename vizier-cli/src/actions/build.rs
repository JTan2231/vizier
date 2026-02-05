use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use vizier_core::config;

use crate::actions::shared::format_table;
use crate::cli::scheduler::{
    background_config_snapshot, build_background_child_args, generate_job_id, user_friendly_args,
};
use crate::{jobs, plan};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildFile {
    steps: Vec<BuildStep>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BuildStep {
    Single(IntentDoc),
    Parallel(Vec<IntentDoc>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentDoc {
    text: Option<String>,
    file: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedIntent {
    text: String,
}

#[derive(Debug)]
struct PlannedJob {
    step_index: usize,
    slug: String,
    branch: String,
    job_id: String,
    input_path: PathBuf,
}

pub(crate) fn run_build(
    build_file: PathBuf,
    global_args: &[String],
    project_root: &Path,
    jobs_root: &Path,
    cli_agent_override: Option<&config::AgentOverrides>,
) -> Result<(), Box<dyn std::error::Error>> {
    let build_path = std::fs::canonicalize(&build_file)
        .map_err(|err| format!("unable to read build file {}: {err}", build_file.display()))?;
    let build_dir = build_path
        .parent()
        .ok_or("build file path must have a parent directory")?;
    let repo_root = std::fs::canonicalize(project_root)?;

    let parsed = parse_build_file(&build_path)?;
    let resolved_steps = resolve_steps(parsed, build_dir, &repo_root)?;
    let planned_steps = plan_steps(&resolved_steps, project_root)?;

    let cfg = config::get_config();
    let config_snapshot = background_config_snapshot(&cfg);
    let binary = std::env::current_exe()?;
    let binary_label = std::env::args()
        .next()
        .unwrap_or_else(|| "vizier".to_string());

    let mut previous_group_artifacts: Vec<jobs::JobArtifact> = Vec::new();
    let mut summary_rows = Vec::new();

    for planned_group in planned_steps {
        let mut group_artifacts = Vec::new();
        for planned in planned_group {
            let raw_args = build_draft_raw_args(
                &binary_label,
                global_args,
                &planned.input_path,
                &planned.slug,
            );
            let child_args = build_background_child_args(
                &raw_args,
                &planned.job_id,
                &cfg.workflow.background,
                false,
                &[],
            );
            let recorded_args = user_friendly_args(&raw_args);

            let schedule = jobs::JobSchedule {
                dependencies: previous_group_artifacts
                    .iter()
                    .cloned()
                    .map(|artifact| jobs::JobDependency { artifact })
                    .collect(),
                locks: vec![
                    jobs::JobLock {
                        key: format!("branch:{}", planned.branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("temp_worktree:{}", planned.job_id),
                        mode: jobs::LockMode::Exclusive,
                    },
                ],
                artifacts: vec![
                    jobs::JobArtifact::PlanBranch {
                        slug: planned.slug.clone(),
                        branch: planned.branch.clone(),
                    },
                    jobs::JobArtifact::PlanDoc {
                        slug: planned.slug.clone(),
                        branch: planned.branch.clone(),
                    },
                ],
                ..Default::default()
            };

            let metadata =
                build_draft_job_metadata(&cfg, cli_agent_override, &planned.slug, &planned.branch);

            jobs::enqueue_job(
                project_root,
                jobs_root,
                &planned.job_id,
                &child_args,
                &recorded_args,
                Some(metadata),
                Some(config_snapshot.clone()),
                Some(schedule),
            )?;

            group_artifacts.push(jobs::JobArtifact::PlanDoc {
                slug: planned.slug.clone(),
                branch: planned.branch.clone(),
            });

            summary_rows.push(vec![
                planned.step_index.to_string(),
                planned.slug.clone(),
                planned.branch.clone(),
                planned.job_id.clone(),
            ]);
        }
        previous_group_artifacts = group_artifacts;
    }

    let _ = jobs::scheduler_tick(project_root, jobs_root, &binary);

    if !summary_rows.is_empty() {
        let mut table = Vec::with_capacity(summary_rows.len() + 1);
        table.push(vec![
            "Step".to_string(),
            "Plan".to_string(),
            "Branch".to_string(),
            "Job".to_string(),
        ]);
        table.extend(summary_rows);
        println!("Outcome: Build queued");
        println!("Steps:");
        println!("{}", format_table(&table, 2));
        println!("Status: vizier jobs list");
        println!("Schedule: vizier jobs schedule");
    }

    Ok(())
}

fn parse_build_file(path: &Path) -> Result<BuildFile, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match extension.as_str() {
        "toml" => toml::from_str(&contents).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "failed to parse TOML build file {}: {err}",
                path.display()
            ))
        }),
        "json" => serde_json::from_str(&contents).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "failed to parse JSON build file {}: {err}",
                path.display()
            ))
        }),
        _ => Err(format!(
            "unsupported build file extension for {} (expected .toml or .json)",
            path.display()
        )
        .into()),
    }
}

fn resolve_steps(
    parsed: BuildFile,
    build_dir: &Path,
    repo_root: &Path,
) -> Result<Vec<Vec<ResolvedIntent>>, Box<dyn std::error::Error>> {
    if parsed.steps.is_empty() {
        return Err("build file steps must be non-empty".into());
    }

    let mut resolved_steps = Vec::new();
    for (idx, step) in parsed.steps.into_iter().enumerate() {
        let step_index = idx + 1;
        match step {
            BuildStep::Single(intent) => {
                let resolved = resolve_intent(intent, build_dir, repo_root, step_index, None)?;
                resolved_steps.push(vec![resolved]);
            }
            BuildStep::Parallel(intents) => {
                if intents.is_empty() {
                    return Err(
                        format!("step {step_index} parallel group must be non-empty").into(),
                    );
                }
                let mut group = Vec::new();
                for (intent_idx, intent) in intents.into_iter().enumerate() {
                    let resolved = resolve_intent(
                        intent,
                        build_dir,
                        repo_root,
                        step_index,
                        Some(intent_idx + 1),
                    )?;
                    group.push(resolved);
                }
                resolved_steps.push(group);
            }
        }
    }

    Ok(resolved_steps)
}

fn resolve_intent(
    intent: IntentDoc,
    build_dir: &Path,
    repo_root: &Path,
    step_index: usize,
    parallel_index: Option<usize>,
) -> Result<ResolvedIntent, Box<dyn std::error::Error>> {
    match (intent.text, intent.file) {
        (Some(text), None) => {
            if text.trim().is_empty() {
                return Err(format!("step {step_index} intent text must be non-empty").into());
            }
            Ok(ResolvedIntent { text })
        }
        (None, Some(path)) => {
            let resolved_path = resolve_intent_path(build_dir, repo_root, &path)?;
            let contents = std::fs::read_to_string(&resolved_path).map_err(|err| {
                format!(
                    "unable to read intent file {}: {err}",
                    resolved_path.display()
                )
            })?;
            if contents.trim().is_empty() {
                return Err(format!(
                    "step {step_index} intent file {} is empty",
                    resolved_path.display()
                )
                .into());
            }
            Ok(ResolvedIntent { text: contents })
        }
        (Some(_), Some(_)) => {
            Err(format!("step {step_index} intent must set exactly one of text or file").into())
        }
        (None, None) => {
            let detail = parallel_index
                .map(|idx| format!("step {step_index} intent {idx}"))
                .unwrap_or_else(|| format!("step {step_index} intent"));
            Err(format!("{detail} must set exactly one of text or file").into())
        }
    }
}

fn resolve_intent_path(
    build_dir: &Path,
    repo_root: &Path,
    raw_path: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let candidate = PathBuf::from(raw_path);
    let joined = if candidate.is_absolute() {
        candidate
    } else {
        build_dir.join(candidate)
    };
    let resolved = std::fs::canonicalize(&joined)
        .map_err(|err| format!("unable to resolve intent file {}: {err}", joined.display()))?;
    if !resolved.starts_with(repo_root) {
        return Err(format!(
            "intent file {} escapes repo root {}",
            resolved.display(),
            repo_root.display()
        )
        .into());
    }
    let metadata = std::fs::metadata(&resolved)?;
    if !metadata.is_file() {
        return Err(format!("intent file {} must be a file", resolved.display()).into());
    }
    Ok(resolved)
}

fn plan_steps(
    resolved_steps: &[Vec<ResolvedIntent>],
    project_root: &Path,
) -> Result<Vec<Vec<PlannedJob>>, Box<dyn std::error::Error>> {
    let plan_dir = project_root.join(plan::PLAN_DIR);
    std::fs::create_dir_all(&plan_dir)?;

    let mut reserved = HashSet::new();
    let mut planned = Vec::new();

    for (step_idx, group) in resolved_steps.iter().enumerate() {
        let mut planned_group = Vec::new();
        for intent in group {
            let base_slug = plan::slug_from_spec(&intent.text);
            let slug = reserve_unique_slug(&base_slug, &plan_dir, "draft/", &mut reserved)?;
            let branch = plan::default_branch_for_slug(&slug);
            let job_id = generate_job_id();
            let input_path = write_job_input(project_root, &job_id, &intent.text)?;

            planned_group.push(PlannedJob {
                step_index: step_idx + 1,
                slug,
                branch,
                job_id,
                input_path,
            });
        }
        planned.push(planned_group);
    }

    Ok(planned)
}

fn reserve_unique_slug(
    base: &str,
    plan_dir: &Path,
    branch_prefix: &str,
    reserved: &mut HashSet<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut attempts = 0usize;
    let mut slug = base.to_string();

    loop {
        let branch_name = format!("{branch_prefix}{slug}");
        let plan_path = plan_dir.join(format!("{slug}.md"));
        let branch_taken = vizier_core::vcs::branch_exists(&branch_name)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        let reserved_taken = reserved.contains(&slug);

        if !branch_taken && !plan_path.exists() && !reserved_taken {
            reserved.insert(slug.clone());
            return Ok(slug);
        }

        attempts += 1;
        if attempts <= 5 {
            slug = plan::normalize_slug(&format!("{base}-{attempts}"));
            if slug.is_empty() {
                slug = plan::normalize_slug(&format!("{base}-{attempts}"));
            }
            continue;
        }

        slug = plan::normalize_slug(&format!("{base}-{}", plan::short_suffix()));
        if slug.is_empty() {
            slug = plan::normalize_slug("draft-plan");
        }

        if attempts > 20 {
            return Err("unable to allocate a unique draft slug after multiple attempts".into());
        }
    }
}

fn write_job_input(
    project_root: &Path,
    job_id: &str,
    text: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = project_root.join(".vizier/tmp/job-inputs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{job_id}.txt"));
    std::fs::write(&path, text)?;
    Ok(path)
}

fn build_draft_raw_args(
    binary: &str,
    global_args: &[String],
    input_path: &Path,
    slug: &str,
) -> Vec<String> {
    let mut args = Vec::new();
    args.push(binary.to_string());
    args.extend(global_args.iter().cloned());
    args.push("draft".to_string());
    args.push("--file".to_string());
    args.push(input_path.display().to_string());
    args.push("--name".to_string());
    args.push(slug.to_string());
    args
}

fn build_draft_job_metadata(
    cfg: &config::Config,
    cli_agent_override: Option<&config::AgentOverrides>,
    slug: &str,
    branch: &str,
) -> jobs::JobMetadata {
    let mut metadata = jobs::JobMetadata {
        background_quiet: Some(cfg.workflow.background.quiet),
        config_backend: Some(cfg.backend.to_string()),
        config_agent_selector: Some(cfg.agent_selector.clone()),
        config_agent_label: cfg.agent_runtime.label.clone(),
        scope: Some(config::CommandScope::Draft.as_str().to_string()),
        plan: Some(slug.to_string()),
        branch: Some(branch.to_string()),
        ..Default::default()
    };

    if !cfg.agent_runtime.command.is_empty() {
        metadata.config_agent_command = Some(cfg.agent_runtime.command.clone());
    }

    if let Ok(agent) =
        config::resolve_agent_settings(cfg, config::CommandScope::Draft, cli_agent_override)
    {
        metadata.agent_selector = Some(agent.selector.clone());
        metadata.agent_backend = Some(agent.backend.to_string());
        metadata.agent_label = Some(agent.agent_runtime.label.clone());
        if !agent.agent_runtime.command.is_empty() {
            metadata.agent_command = Some(agent.agent_runtime.command.clone());
        }
    }

    metadata
}
