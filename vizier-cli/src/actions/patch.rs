use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use vizier_core::{config, vcs::branch_exists};

use super::build::{BuildExecuteArgs, BuildExecutionPipeline, run_build, run_build_execute};
use super::shared::{append_agent_rows, current_verbosity, format_block, require_agent_backend};
use super::types::CommitMode;

#[derive(Debug, Clone)]
pub(crate) struct PatchArgs {
    pub files: Vec<PathBuf>,
    pub pipeline: Option<BuildExecutionPipeline>,
    pub target: Option<String>,
    pub resume: bool,
    pub assume_yes: bool,
    pub follow: bool,
    pub after: Vec<String>,
}

#[derive(Debug, Clone)]
struct PatchInput {
    repo_rel: String,
    abs: PathBuf,
}

fn pipeline_label(pipeline: Option<BuildExecutionPipeline>) -> &'static str {
    match pipeline {
        Some(BuildExecutionPipeline::Approve) => "approve",
        Some(BuildExecutionPipeline::ApproveReview) => "approve-review",
        Some(BuildExecutionPipeline::ApproveReviewMerge) => "approve-review-merge",
        None => "default",
    }
}

fn digest_short(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let hex = format!("{:x}", hasher.finalize());
    hex.chars().take(12).collect()
}

fn patch_session_id(
    inputs: &[PatchInput],
    pipeline: Option<BuildExecutionPipeline>,
    target: Option<&str>,
) -> String {
    let mut seed = String::new();
    seed.push_str(pipeline_label(pipeline));
    seed.push('\n');
    seed.push_str(target.unwrap_or("default"));
    seed.push('\n');
    for input in inputs {
        seed.push_str(&input.repo_rel);
        seed.push('\n');
    }
    format!("patch-{}", digest_short(&seed))
}

fn toml_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn step_key(index: usize) -> String {
    format!("{:02}", index + 1)
}

fn preflight_patch_inputs(
    files: &[PathBuf],
    repo_root: &Path,
) -> Result<(Vec<PatchInput>, usize), Box<dyn std::error::Error>> {
    if files.is_empty() {
        return Err("vizier patch requires at least one file".into());
    }

    let mut accepted = Vec::new();
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    let mut duplicate_count = 0usize;

    for raw in files {
        let canonical = match std::fs::canonicalize(raw) {
            Ok(value) => value,
            Err(err) => {
                errors.push(format!("{}: {err}", raw.display()));
                continue;
            }
        };
        if !canonical.starts_with(repo_root) {
            errors.push(format!(
                "{} resolves outside the repository root {}",
                canonical.display(),
                repo_root.display()
            ));
            continue;
        }
        let metadata = match std::fs::metadata(&canonical) {
            Ok(value) => value,
            Err(err) => {
                errors.push(format!("{}: {err}", canonical.display()));
                continue;
            }
        };
        if !metadata.is_file() {
            errors.push(format!("{} is not a file", canonical.display()));
            continue;
        }
        let contents = match std::fs::read_to_string(&canonical) {
            Ok(value) => value,
            Err(err) => {
                errors.push(format!(
                    "{} is not readable UTF-8 text: {err}",
                    canonical.display()
                ));
                continue;
            }
        };
        if contents.trim().is_empty() {
            errors.push(format!("{} is empty", canonical.display()));
            continue;
        }

        let key = canonical.to_string_lossy().to_string();
        if !seen.insert(key) {
            duplicate_count += 1;
            continue;
        }

        let repo_rel = canonical
            .strip_prefix(repo_root)
            .map_err(|_| "failed to relativize patch input path")?
            .to_string_lossy()
            .replace('\\', "/");
        accepted.push(PatchInput {
            repo_rel,
            abs: canonical,
        });
    }

    if !errors.is_empty() {
        let mut message = String::from("patch preflight failed:\n");
        for entry in errors {
            message.push_str(" - ");
            message.push_str(&entry);
            message.push('\n');
        }
        return Err(message.trim_end().to_string().into());
    }

    if accepted.is_empty() {
        return Err("patch preflight failed: no usable files remained after dedupe".into());
    }

    Ok((accepted, duplicate_count))
}

fn write_patch_build_file(
    repo_root: &Path,
    build_id: &str,
    inputs: &[PatchInput],
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let patches_dir = repo_root.join(".vizier/tmp/patches");
    std::fs::create_dir_all(&patches_dir)?;
    let build_file = patches_dir.join(format!("{build_id}.toml"));

    let mut body = String::new();
    body.push_str("steps = [\n");
    for (idx, input) in inputs.iter().enumerate() {
        let file_path = toml_escape(&input.abs.to_string_lossy());
        if idx == 0 {
            body.push_str(&format!("  {{ file = \"{file_path}\" }},\n"));
        } else {
            let prev_step = step_key(idx - 1);
            body.push_str(&format!(
                "  {{ file = \"{file_path}\", after_steps = [\"{prev_step}\"] }},\n"
            ));
        }
    }
    body.push_str("]\n");

    std::fs::write(&build_file, body)?;
    Ok(build_file)
}

pub(crate) async fn run_patch(
    args: PatchArgs,
    project_root: &Path,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::ImplementationPlan,
        "vizier patch requires an agent-capable selector; update [agents.draft] or pass --agent codex|gemini",
    )?;

    let repo_root = std::fs::canonicalize(project_root)?;
    let (inputs, duplicate_count) = preflight_patch_inputs(&args.files, &repo_root)?;
    let build_id = patch_session_id(&inputs, args.pipeline, args.target.as_deref());
    let build_branch = format!("build/{build_id}");
    let build_exists = branch_exists(&build_branch)
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?
        || repo_root
            .join(".vizier/implementation-plans/builds")
            .join(&build_id)
            .exists();

    if args.resume && !build_exists {
        return Err(
            format!("patch session {build_id} not found; rerun without --resume first").into(),
        );
    }
    if !args.resume && build_exists {
        return Err(format!("patch session {build_id} already exists; rerun with --resume").into());
    }

    let build_file = write_patch_build_file(&repo_root, &build_id, &inputs)?;

    let mut preflight_rows = Vec::new();
    preflight_rows.push(("Outcome".to_string(), "Patch preflight passed".to_string()));
    preflight_rows.push(("Patch session".to_string(), build_id.clone()));
    preflight_rows.push(("Files".to_string(), format!("{}", inputs.len())));
    preflight_rows.push((
        "Pipeline".to_string(),
        pipeline_label(args.pipeline).to_string(),
    ));
    preflight_rows.push((
        "Target".to_string(),
        args.target.clone().unwrap_or_else(|| "default".to_string()),
    ));
    preflight_rows.push((
        "Execution manifest".to_string(),
        format!(".vizier/implementation-plans/builds/{build_id}/execution.json"),
    ));
    if duplicate_count > 0 {
        preflight_rows.push(("Deduplicated".to_string(), duplicate_count.to_string()));
    }
    if !args.after.is_empty() {
        preflight_rows.push(("After".to_string(), args.after.join(",")));
    }
    append_agent_rows(&mut preflight_rows, current_verbosity());
    println!("{}", format_block(preflight_rows));

    println!("Patch queue:");
    for (idx, input) in inputs.iter().enumerate() {
        println!("  {:>2}. {}", idx + 1, input.repo_rel);
    }

    if !args.resume {
        run_build(
            build_file,
            Some(build_id.clone()),
            &repo_root,
            agent,
            commit_mode,
        )
        .await?;
    }

    run_build_execute(
        BuildExecuteArgs {
            build_id,
            pipeline_override: args.pipeline,
            target_override: args.target,
            resume: args.resume,
            assume_yes: args.assume_yes,
            follow: args.follow,
            requested_after: &args.after,
        },
        &repo_root,
    )
    .await
}
