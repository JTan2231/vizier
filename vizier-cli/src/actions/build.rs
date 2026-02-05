use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vizier_core::{
    agent_prompt,
    auditor::Auditor,
    config,
    display::{self, LogLevel},
    vcs::{
        add_worktree_for_branch, branch_exists, commit_paths_in_repo, create_branch_from,
        detect_primary_branch, remove_worktree,
    },
};

use crate::actions::shared::{
    WorkdirGuard, append_agent_rows, copy_session_log_to_repo_root, current_verbosity,
    format_block, format_table, prompt_selection, require_agent_backend,
};
use crate::actions::types::CommitMode;
use crate::{jobs, plan};

const BUILD_PLAN_ROOT: &str = ".vizier/implementation-plans/builds";

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
enum IntentSource {
    Text,
    File(PathBuf),
}

impl IntentSource {
    fn as_manifest_value(&self) -> String {
        match self {
            Self::Text => "text".to_string(),
            Self::File(path) => format!("file:{}", path.display()),
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedIntent {
    text: String,
    source: IntentSource,
}

#[derive(Debug, Clone)]
struct NormalizedStep {
    stage_index: usize,
    parallel_index: Option<usize>,
    step_key: String,
    slug: String,
    file_name: String,
    intent: ResolvedIntent,
}

#[derive(Debug, Clone, Serialize)]
struct BuildManifest {
    build_id: String,
    created_at: String,
    target_branch: String,
    build_branch: String,
    input_file: ManifestInputFile,
    steps: Vec<ManifestStep>,
    artifacts: ManifestArtifacts,
    status: ManifestStatus,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestInputFile {
    original_path: String,
    copied_path: String,
    digest: String,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestArtifacts {
    plan_docs: Vec<String>,
    summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestStep {
    step_key: String,
    stage_index: usize,
    parallel_index: Option<usize>,
    intent_source: String,
    intent_digest: String,
    output_plan_path: String,
    reads: Vec<ManifestPlanReference>,
    result: ManifestStepResult,
    summary: Option<String>,
    output_digest: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestPlanReference {
    step_key: String,
    plan_path: String,
    summary: String,
    digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ManifestStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ManifestStepResult {
    Pending,
    Succeeded,
    Failed,
}

impl ManifestStepResult {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

pub(crate) async fn run_build(
    build_file: PathBuf,
    project_root: &Path,
    agent: &config::AgentSettings,
    commit_mode: CommitMode,
) -> Result<(), Box<dyn std::error::Error>> {
    require_agent_backend(
        agent,
        config::PromptKind::ImplementationPlan,
        "vizier build requires an agent-capable selector; update [agents.draft] or pass --agent codex|gemini",
    )?;

    let build_path = std::fs::canonicalize(&build_file)
        .map_err(|err| format!("unable to read build file {}: {err}", build_file.display()))?;
    let build_dir = build_path
        .parent()
        .ok_or("build file path must have a parent directory")?;
    let repo_root = std::fs::canonicalize(project_root)?;

    let _repo_guard = WorkdirGuard::enter(&repo_root)?;

    let (parsed, build_contents) = parse_build_file(&build_path)?;
    let resolved_steps = resolve_steps(parsed, build_dir, &repo_root)?;
    let normalized_steps = normalize_steps(&resolved_steps);
    if normalized_steps.is_empty() {
        return Err("build file steps must be non-empty".into());
    }

    let target_branch = detect_primary_branch()
        .ok_or("unable to detect a primary branch (tried origin/HEAD, main, master)")?;
    let build_id = allocate_build_id(&build_path, &build_contents, &repo_root)?;
    let build_branch = format!("build/{build_id}");

    let tmp_root = repo_root.join(".vizier/tmp-worktrees");
    std::fs::create_dir_all(&tmp_root)?;
    let worktree_name = format!("vizier-build-{build_id}");
    let worktree_path = tmp_root.join(format!("build-{build_id}"));

    create_branch_from(&target_branch, &build_branch).map_err(
        |err| -> Box<dyn std::error::Error> {
            Box::from(format!(
                "create_branch_from({}<-{}): {}",
                build_branch, target_branch, err
            ))
        },
    )?;

    add_worktree_for_branch(&worktree_name, &worktree_path, &build_branch).map_err(
        |err| -> Box<dyn std::error::Error> {
            Box::from(format!(
                "add_worktree({}, {}): {}",
                worktree_name,
                worktree_path.display(),
                err
            ))
        },
    )?;
    jobs::record_current_job_worktree(&repo_root, Some(&worktree_name), &worktree_path);

    let session_rel = Path::new(BUILD_PLAN_ROOT).join(&build_id);
    let input_name = build_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("build.toml");
    let input_rel = session_rel.join("input").join(input_name);
    let plans_rel_root = session_rel.join("plans");
    let manifest_rel = session_rel.join("manifest.json");
    let summary_rel = session_rel.join("summary.md");

    let mut artifact_paths = vec![input_rel.clone(), manifest_rel.clone(), summary_rel.clone()];
    for step in &normalized_steps {
        artifact_paths.push(plans_rel_root.join(&step.file_name));
    }

    let mut failed_step_key: Option<String> = None;
    let mut failure_reason: Option<String> = None;
    let mut committed = false;

    let final_manifest: Option<BuildManifest> = {
        let _worktree_guard = WorkdirGuard::enter(&worktree_path)?;

        let session_abs = worktree_path.join(&session_rel);
        let input_abs = worktree_path.join(&input_rel);
        let plans_abs = worktree_path.join(&plans_rel_root);
        let manifest_abs = worktree_path.join(&manifest_rel);
        let summary_abs = worktree_path.join(&summary_rel);

        if let Some(parent) = input_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&plans_abs)?;

        std::fs::write(&input_abs, &build_contents)?;

        let input_digest = digest_hex(build_contents.as_bytes());
        let mut manifest = build_manifest_template(
            BuildManifestInit {
                build_id: &build_id,
                target_branch: &target_branch,
                build_branch: &build_branch,
                input_original: &build_path,
                input_copied: &input_rel,
                input_digest: &input_digest,
                plans_rel_root: &plans_rel_root,
            },
            &normalized_steps,
        );
        write_manifest(&manifest_abs, &manifest)?;

        let prompt_agent = agent.for_prompt(config::PromptKind::ImplementationPlan)?;
        let selection = prompt_selection(&prompt_agent)?;

        display::emit(
            LogLevel::Info,
            format!(
                "[build] session={} branch={} manifest={}",
                build_id,
                build_branch,
                manifest_rel.display()
            ),
        );

        for (step_idx, step) in normalized_steps.iter().enumerate() {
            let reads = prior_stage_refs(&manifest, step.stage_index);
            manifest.steps[step_idx].reads = reads.clone();
            if let Err(err) = write_manifest(&manifest_abs, &manifest) {
                let message = format!("write manifest before step {}: {err}", step.step_key);
                mark_step_failed(&mut manifest, step_idx, &message);
                failure_reason = Some(message);
                failed_step_key = Some(step.step_key.clone());
                let _ = write_manifest(&manifest_abs, &manifest);
                break;
            }

            let output_rel = plans_rel_root.join(&step.file_name);
            let output_abs = worktree_path.join(&output_rel);

            let prompt_refs = reads
                .iter()
                .map(|entry| agent_prompt::BuildPlanReference {
                    step_key: entry.step_key.clone(),
                    plan_path: entry.plan_path.clone(),
                    summary: entry.summary.clone(),
                    digest: entry.digest.clone(),
                })
                .collect::<Vec<_>>();

            let prompt = match agent_prompt::build_build_implementation_plan_prompt(
                selection,
                agent_prompt::BuildPlanPromptInput {
                    build_id: &build_id,
                    build_branch: &build_branch,
                    manifest_path: &to_repo_string(&manifest_rel),
                    step_key: &step.step_key,
                    stage_index: step.stage_index,
                    parallel_index: step.parallel_index,
                    output_plan_path: &to_repo_string(&output_rel),
                    intent_text: &step.intent.text,
                    references: &prompt_refs,
                    documentation: &prompt_agent.documentation,
                },
            ) {
                Ok(value) => value,
                Err(err) => {
                    let message = format!(
                        "build prompt assembly failed for step {}: {}",
                        step.step_key, err
                    );
                    mark_step_failed(&mut manifest, step_idx, &message);
                    failure_reason = Some(message);
                    failed_step_key = Some(step.step_key.clone());
                    let _ = write_manifest(&manifest_abs, &manifest);
                    break;
                }
            };

            display::emit(
                LogLevel::Info,
                format!(
                    "[build] step={} status=running plan={}",
                    step.step_key,
                    output_rel.display()
                ),
            );

            let llm_response = match Auditor::llm_request_with_tools(
                &prompt_agent,
                Some(config::PromptKind::ImplementationPlan),
                prompt,
                step.intent.text.clone(),
                Some(worktree_path.clone()),
            )
            .await
            {
                Ok(response) => response,
                Err(err) => {
                    let message = format!("Agent backend for step {}: {}", step.step_key, err);
                    mark_step_failed(&mut manifest, step_idx, &message);
                    failure_reason = Some(message);
                    failed_step_key = Some(step.step_key.clone());
                    let _ = write_manifest(&manifest_abs, &manifest);
                    break;
                }
            };

            let plan_body = llm_response.content;
            let plan_doc = render_build_plan_document(step, &build_branch, &plan_body);
            if let Err(err) = std::fs::write(&output_abs, &plan_doc) {
                let message = format!(
                    "write plan for step {} to {}: {}",
                    step.step_key,
                    output_rel.display(),
                    err
                );
                mark_step_failed(&mut manifest, step_idx, &message);
                failure_reason = Some(message);
                failed_step_key = Some(step.step_key.clone());
                let _ = write_manifest(&manifest_abs, &manifest);
                break;
            }

            let summary = summarize_text(&plan_body);
            let output_digest = digest_hex(plan_doc.as_bytes());
            if let Some(step_entry) = manifest.steps.get_mut(step_idx) {
                step_entry.result = ManifestStepResult::Succeeded;
                step_entry.summary = Some(summary.clone());
                step_entry.output_digest = Some(output_digest);
                step_entry.error = None;
            }

            if !manifest
                .artifacts
                .plan_docs
                .iter()
                .any(|entry| entry == &to_repo_string(&output_rel))
            {
                manifest
                    .artifacts
                    .plan_docs
                    .push(to_repo_string(&output_rel));
            }

            if let Err(err) = write_manifest(&manifest_abs, &manifest) {
                let message = format!("write manifest after step {}: {err}", step.step_key);
                mark_step_failed(&mut manifest, step_idx, &message);
                failure_reason = Some(message);
                failed_step_key = Some(step.step_key.clone());
                let _ = write_manifest(&manifest_abs, &manifest);
                break;
            }

            display::emit(
                LogLevel::Info,
                format!(
                    "[build] step={} status=succeeded plan={}",
                    step.step_key,
                    output_rel.display()
                ),
            );
        }

        manifest.status = if failure_reason.is_some() {
            ManifestStatus::Failed
        } else {
            ManifestStatus::Succeeded
        };

        let summary_document = render_session_summary(&manifest);
        if let Err(err) = std::fs::write(&summary_abs, summary_document) {
            let message = format!("write session summary {}: {err}", summary_rel.display());
            if failure_reason.is_none() {
                failure_reason = Some(message);
            }
            manifest.status = ManifestStatus::Failed;
        } else {
            manifest.artifacts.summary = Some(to_repo_string(&summary_rel));
        }

        if let Err(err) = write_manifest(&manifest_abs, &manifest) {
            let message = format!("write final manifest {}: {err}", manifest_rel.display());
            if failure_reason.is_none() {
                failure_reason = Some(message);
            }
            manifest.status = ManifestStatus::Failed;
        }

        if commit_mode.should_commit() {
            let commit_message = if manifest.status == ManifestStatus::Succeeded {
                format!("docs: add build session {}", build_id)
            } else {
                format!("docs: record failed build session {}", build_id)
            };
            match commit_session_artifacts(&worktree_path, &artifact_paths, &commit_message) {
                Ok(()) => {
                    committed = true;
                }
                Err(err) => {
                    let message = format!("commit build session artifacts: {err}");
                    if failure_reason.is_none() {
                        failure_reason = Some(message);
                    }
                    manifest.status = ManifestStatus::Failed;
                    let _ = write_manifest(&manifest_abs, &manifest);
                }
            }
        }

        if failure_reason.is_some() && failed_step_key.is_none() {
            failed_step_key = manifest
                .steps
                .iter()
                .find(|step| step.result == ManifestStepResult::Failed)
                .map(|step| step.step_key.clone());
        }

        if !session_abs.exists() {
            let message = format!(
                "build session path missing unexpectedly: {}",
                session_rel.display()
            );
            if failure_reason.is_none() {
                failure_reason = Some(message);
            }
        }

        Some(manifest)
    };

    if let Some(artifact) = Auditor::persist_session_log() {
        copy_session_log_to_repo_root(&repo_root, &artifact);
        Auditor::clear_messages();
    }

    if failure_reason.is_none() && commit_mode.should_commit() {
        cleanup_worktree(&worktree_name, &worktree_path);
    }

    let mut rows = Vec::new();
    let outcome = if failure_reason.is_none() {
        if commit_mode.should_commit() {
            "Build session ready"
        } else {
            "Build session pending (manual commit)"
        }
    } else {
        "Build session failed"
    };
    rows.push(("Outcome".to_string(), outcome.to_string()));
    rows.push(("Build".to_string(), build_id.clone()));
    rows.push(("Branch".to_string(), build_branch.clone()));
    rows.push(("Manifest".to_string(), to_repo_string(&manifest_rel)));

    if !commit_mode.should_commit() || failure_reason.is_some() {
        rows.push(("Worktree".to_string(), worktree_path.display().to_string()));
    }

    if let Some(step_key) = failed_step_key.as_ref() {
        rows.push(("Failed step".to_string(), step_key.clone()));
    }

    append_agent_rows(&mut rows, current_verbosity());
    println!("{}", format_block(rows));

    if let Some(manifest) = final_manifest.as_ref() {
        let mut table = vec![vec![
            "Step".to_string(),
            "Status".to_string(),
            "Plan".to_string(),
            "Reads".to_string(),
        ]];

        for step in &manifest.steps {
            table.push(vec![
                step.step_key.clone(),
                step.result.label().to_string(),
                step.output_plan_path.clone(),
                step.reads.len().to_string(),
            ]);
        }

        println!("Steps:");
        println!("{}", format_table(&table, 2));
    }

    if let Some(reason) = failure_reason {
        display::emit(
            LogLevel::Error,
            format!(
                "Build session artifacts preserved on {} (worktree {}).",
                build_branch,
                worktree_path.display()
            ),
        );
        return Err(reason.into());
    }

    if !commit_mode.should_commit() {
        display::info(format!(
            "Build session generated with --no-commit; inspect and commit in {}",
            worktree_path.display()
        ));
    }

    if committed {
        display::info(format!(
            "Build session artifacts committed on {}; inspect with `git checkout {}`",
            build_branch, build_branch
        ));
    }

    Ok(())
}

struct BuildManifestInit<'a> {
    build_id: &'a str,
    target_branch: &'a str,
    build_branch: &'a str,
    input_original: &'a Path,
    input_copied: &'a Path,
    input_digest: &'a str,
    plans_rel_root: &'a Path,
}

fn build_manifest_template(init: BuildManifestInit<'_>, steps: &[NormalizedStep]) -> BuildManifest {
    let manifest_steps = steps
        .iter()
        .map(|step| ManifestStep {
            step_key: step.step_key.clone(),
            stage_index: step.stage_index,
            parallel_index: step.parallel_index,
            intent_source: step.intent.source.as_manifest_value(),
            intent_digest: digest_hex(step.intent.text.as_bytes()),
            output_plan_path: to_repo_string(&init.plans_rel_root.join(&step.file_name)),
            reads: Vec::new(),
            result: ManifestStepResult::Pending,
            summary: None,
            output_digest: None,
            error: None,
        })
        .collect::<Vec<_>>();

    BuildManifest {
        build_id: init.build_id.to_string(),
        created_at: Utc::now().to_rfc3339(),
        target_branch: init.target_branch.to_string(),
        build_branch: init.build_branch.to_string(),
        input_file: ManifestInputFile {
            original_path: init.input_original.display().to_string(),
            copied_path: to_repo_string(init.input_copied),
            digest: init.input_digest.to_string(),
        },
        steps: manifest_steps,
        artifacts: ManifestArtifacts {
            plan_docs: Vec::new(),
            summary: None,
        },
        status: ManifestStatus::Running,
    }
}

fn mark_step_failed(manifest: &mut BuildManifest, step_idx: usize, error: &str) {
    if let Some(entry) = manifest.steps.get_mut(step_idx) {
        entry.result = ManifestStepResult::Failed;
        entry.error = Some(error.to_string());
    }
    manifest.status = ManifestStatus::Failed;
}

fn prior_stage_refs(manifest: &BuildManifest, current_stage: usize) -> Vec<ManifestPlanReference> {
    manifest
        .steps
        .iter()
        .filter(|step| step.stage_index < current_stage)
        .filter(|step| step.result == ManifestStepResult::Succeeded)
        .map(|step| ManifestPlanReference {
            step_key: step.step_key.clone(),
            plan_path: step.output_plan_path.clone(),
            summary: step
                .summary
                .clone()
                .unwrap_or_else(|| "(summary unavailable)".to_string()),
            digest: step.output_digest.clone(),
        })
        .collect()
}

fn commit_session_artifacts(
    worktree_path: &Path,
    artifact_paths: &[PathBuf],
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let existing = artifact_paths
        .iter()
        .filter(|path| worktree_path.join(path).is_file())
        .cloned()
        .collect::<Vec<_>>();

    if existing.is_empty() {
        return Err("no build session artifacts found for commit".into());
    }

    let refs = existing
        .iter()
        .map(|path| path.as_path())
        .collect::<Vec<_>>();
    commit_paths_in_repo(worktree_path, &refs, message)
        .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
    Ok(())
}

fn cleanup_worktree(worktree_name: &str, worktree_path: &Path) {
    if let Err(err) = remove_worktree(worktree_name, true) {
        display::warn(format!(
            "temporary build worktree cleanup failed ({}); remove manually with `git worktree prune`",
            err
        ));
    }
    if worktree_path.exists() {
        let _ = std::fs::remove_dir_all(worktree_path);
    }
}

fn render_build_plan_document(
    step: &NormalizedStep,
    build_branch: &str,
    plan_body: &str,
) -> String {
    let plan_slug = format!("{}-{}", step.step_key, step.slug);
    plan::render_plan_document(&plan_slug, build_branch, &step.intent.text, plan_body)
}

fn render_session_summary(manifest: &BuildManifest) -> String {
    let mut out = String::new();
    out.push_str("# Build Session Summary\n\n");
    out.push_str(&format!("- Build: `{}`\n", manifest.build_id));
    out.push_str(&format!("- Branch: `{}`\n", manifest.build_branch));
    out.push_str(&format!("- Target: `{}`\n", manifest.target_branch));
    out.push_str(&format!("- Status: `{}`\n", manifest.status.as_label()));
    out.push_str("- Manifest: `manifest.json`\n");
    out.push_str("\n## Steps\n\n");

    for step in &manifest.steps {
        out.push_str(&format!(
            "- `{}` `{}` -> `{}`",
            step.step_key,
            step.result.label(),
            step.output_plan_path
        ));
        if !step.reads.is_empty() {
            let refs = step
                .reads
                .iter()
                .map(|entry| entry.step_key.clone())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(" (reads: {})", refs));
        }
        if let Some(error) = step.error.as_ref() {
            out.push_str(&format!(" (error: {})", summarize_text(error)));
        }
        out.push('\n');
    }

    out
}

impl ManifestStatus {
    fn as_label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

fn summarize_text(input: &str) -> String {
    let first_line = input
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("(summary unavailable)");

    truncate_chars(first_line, 140)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }

    let mut clipped = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= max_chars.saturating_sub(1) {
            clipped.push('…');
            break;
        }
        clipped.push(ch);
    }

    clipped
}

fn parse_build_file(path: &Path) -> Result<(BuildFile, String), Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let parsed = match extension.as_str() {
        "toml" => toml::from_str(&contents).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "failed to parse TOML build file {}: {err}",
                path.display()
            ))
        })?,
        "json" => serde_json::from_str(&contents).map_err(|err| {
            Box::<dyn std::error::Error>::from(format!(
                "failed to parse JSON build file {}: {err}",
                path.display()
            ))
        })?,
        _ => {
            return Err(format!(
                "unsupported build file extension for {} (expected .toml or .json)",
                path.display()
            )
            .into());
        }
    };

    Ok((parsed, contents))
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
            Ok(ResolvedIntent {
                text,
                source: IntentSource::Text,
            })
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
            Ok(ResolvedIntent {
                text: contents,
                source: IntentSource::File(resolved_path),
            })
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

fn normalize_steps(resolved_steps: &[Vec<ResolvedIntent>]) -> Vec<NormalizedStep> {
    let mut normalized = Vec::new();

    for (stage_idx, group) in resolved_steps.iter().enumerate() {
        let stage_index = stage_idx + 1;
        let parallel = group.len() > 1;
        for (idx, intent) in group.iter().enumerate() {
            let parallel_index = if parallel { Some(idx + 1) } else { None };
            let step_key = build_step_key(stage_index, parallel_index);
            let slug = plan::slug_from_spec(&intent.text);
            let file_name = format!("{step_key}-{slug}.md");
            normalized.push(NormalizedStep {
                stage_index,
                parallel_index,
                step_key,
                slug,
                file_name,
                intent: intent.clone(),
            });
        }
    }

    normalized
}

fn build_step_key(stage_index: usize, parallel_index: Option<usize>) -> String {
    let prefix = format!("{stage_index:02}");
    match parallel_index {
        Some(index) => format!("{prefix}{}", alpha_suffix(index)),
        None => prefix,
    }
}

fn alpha_suffix(mut index: usize) -> String {
    let mut chars = Vec::new();
    while index > 0 {
        index -= 1;
        chars.push((b'a' + (index % 26) as u8) as char);
        index /= 26;
    }
    chars.iter().rev().collect()
}

fn write_manifest(path: &Path, manifest: &BuildManifest) -> Result<(), Box<dyn std::error::Error>> {
    let contents = serde_json::to_string_pretty(manifest)?;
    std::fs::write(path, contents)?;
    Ok(())
}

fn allocate_build_id(
    build_path: &Path,
    build_contents: &str,
    repo_root: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let stem = build_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("build-session");
    let normalized_stem = {
        let value = plan::normalize_slug(stem);
        if value.is_empty() {
            "build-session".to_string()
        } else {
            value
        }
    };

    let digest = short_digest(build_contents.as_bytes());
    let base = format!("{normalized_stem}-{digest}");
    let builds_root = repo_root.join(BUILD_PLAN_ROOT);

    for suffix in 0..128 {
        let candidate = if suffix == 0 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        let branch = format!("build/{candidate}");
        let branch_taken = branch_exists(&branch)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
        let path_taken = builds_root.join(&candidate).exists();
        if !branch_taken && !path_taken {
            return Ok(candidate);
        }
    }

    Err("unable to allocate a unique build session id".into())
}

fn digest_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn short_digest(data: &[u8]) -> String {
    let digest = digest_hex(data);
    digest.chars().take(12).collect()
}

fn to_repo_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{alpha_suffix, build_step_key, normalize_steps};
    use crate::actions::build::{IntentSource, ResolvedIntent};

    #[test]
    fn step_key_formats_serial_and_parallel() {
        assert_eq!(build_step_key(1, None), "01");
        assert_eq!(build_step_key(2, Some(1)), "02a");
        assert_eq!(build_step_key(2, Some(2)), "02b");
        assert_eq!(build_step_key(9, Some(27)), "09aa");
    }

    #[test]
    fn alpha_suffix_rolls_past_z() {
        assert_eq!(alpha_suffix(1), "a");
        assert_eq!(alpha_suffix(26), "z");
        assert_eq!(alpha_suffix(27), "aa");
        assert_eq!(alpha_suffix(52), "az");
        assert_eq!(alpha_suffix(53), "ba");
    }

    #[test]
    fn normalized_steps_are_deterministic() {
        let steps = vec![
            vec![ResolvedIntent {
                text: "Alpha scope".to_string(),
                source: IntentSource::Text,
            }],
            vec![
                ResolvedIntent {
                    text: "Bravo scope".to_string(),
                    source: IntentSource::Text,
                },
                ResolvedIntent {
                    text: "Charlie scope".to_string(),
                    source: IntentSource::Text,
                },
            ],
        ];

        let normalized = normalize_steps(&steps);
        let keys = normalized
            .iter()
            .map(|entry| entry.step_key.clone())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["01", "02a", "02b"]);

        let names = normalized
            .iter()
            .map(|entry| entry.file_name.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "01-alpha-scope.md",
                "02a-bravo-scope.md",
                "02b-charlie-scope.md"
            ]
        );
    }
}
