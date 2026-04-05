use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;
use vizier_core::display;

use crate::actions::shared::format_block;
use crate::actions::workflow_preflight::{
    PreparedWorkflowInvocation, prepare_workflow_invocation, prepare_workflow_template,
    prepare_workflow_template_from_invocation,
};
use crate::cli::args::{RunCmd, RunFormatArg};
use crate::jobs;
use crate::workflow_templates::ResolvedWorkflowSource;

const RUN_AFTER_PREFIX: &str = "run:";
const RUN_ID_PREFIX: &str = "run_";

#[derive(Debug)]
enum AfterReference {
    JobId(String),
    RunId(String),
}

#[derive(Debug, Deserialize, Default)]
struct AfterDependencyRunManifest {
    #[serde(default)]
    nodes: BTreeMap<String, AfterDependencyRunNode>,
}

#[derive(Debug, Deserialize, Default)]
struct AfterDependencyRunNode {
    #[serde(default)]
    job_id: String,
    #[serde(default)]
    routes: AfterDependencyRunRoutes,
}

#[derive(Debug, Deserialize, Default)]
struct AfterDependencyRunRoutes {
    #[serde(default)]
    succeeded: Vec<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct EnqueuedRunSummary {
    index: u32,
    run_id: String,
    enqueue: jobs::EnqueueWorkflowRunResult,
    job_ids: Vec<String>,
    root_jobs: Vec<String>,
    batch: Option<BatchRunItemMetadata>,
}

#[derive(Debug, Clone)]
struct FollowedRunSummary {
    index: u32,
    run_id: String,
    terminal: FollowResult,
    batch: Option<BatchRunItemMetadata>,
}

#[derive(Debug, Clone)]
struct BatchRunItemMetadata {
    spec_file: String,
    slug: String,
}

#[derive(Debug, Clone)]
struct PreparedRunItem {
    index: u32,
    template: vizier_core::workflow_template::WorkflowTemplate,
    batch: Option<BatchRunItemMetadata>,
}

#[derive(Debug, Clone)]
struct PreparedBatchRun {
    batch_dir: String,
    items: Vec<PreparedRunItem>,
}

#[derive(Debug, Clone)]
enum MultiRunMode {
    Repeat {
        repeat: u32,
    },
    Batch {
        batch_dir: String,
        spec_count: usize,
    },
}

#[derive(Debug, Clone)]
struct DiscoveredBatchSpec {
    absolute_path: PathBuf,
    spec_file: String,
}

pub(crate) fn run_workflow(
    project_root: &Path,
    jobs_root: &Path,
    cmd: RunCmd,
    vizier_root_existed_before_runtime: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = vizier_core::config::get_config();
    let approval_override = if cmd.require_approval {
        Some(true)
    } else if cmd.no_require_approval {
        Some(false)
    } else {
        None
    };
    let binary = std::env::current_exe()?;
    let invocation_args = std::env::args().collect::<Vec<_>>();

    if let Some(spec_dir) = cmd.spec_dir.as_ref() {
        let prepared =
            prepare_workflow_invocation(project_root, &cmd.flow, &cmd.inputs, &cmd.set, &cfg)?;
        let batch = prepare_batch_run(project_root, spec_dir, &prepared)?;
        let first_template = batch
            .items
            .first()
            .map(|item| &item.template)
            .ok_or("batch discovery returned no items")?;

        if cmd.check {
            emit_validation_summary(cmd.format, &prepared.source, first_template, Some(&batch))?;
            return Ok(());
        }

        let summaries = enqueue_serial_runs(
            project_root,
            jobs_root,
            &prepared.source,
            &batch.items,
            &cmd.after,
            approval_override,
            &binary,
            &invocation_args,
            cmd.ephemeral,
            vizier_root_existed_before_runtime,
        )?;
        let mode = MultiRunMode::Batch {
            batch_dir: batch.batch_dir.clone(),
            spec_count: batch.items.len(),
        };

        if !cmd.follow {
            emit_multi_enqueue_summary(
                cmd.format,
                &prepared.source,
                &mode,
                &summaries,
                cmd.ephemeral,
            )?;
            return Ok(());
        }

        let (followed_runs, terminal_state, exit_code) = follow_serial_runs(
            project_root,
            jobs_root,
            &binary,
            &summaries,
            cmd.ephemeral,
            cmd.format,
        )?;
        emit_multi_follow_summary(
            cmd.format,
            &prepared.source,
            &mode,
            &summaries,
            &followed_runs,
            cmd.ephemeral,
            &terminal_state,
            exit_code,
        )?;

        if exit_code == 0 {
            return Ok(());
        }
        std::process::exit(exit_code);
    }

    let prepared = prepare_workflow_template(project_root, &cmd.flow, &cmd.inputs, &cmd.set, &cfg)?;
    let source = prepared.source;
    let template = prepared.template;

    if cmd.check {
        jobs::validate_workflow_run_template(&template)?;
        emit_validation_summary(cmd.format, &source, &template, None)?;
        return Ok(());
    }

    let repeat = cmd.repeat.get();
    let items = (1..=repeat)
        .map(|index| PreparedRunItem {
            index,
            template: template.clone(),
            batch: None,
        })
        .collect::<Vec<_>>();
    let summaries = enqueue_serial_runs(
        project_root,
        jobs_root,
        &source,
        &items,
        &cmd.after,
        approval_override,
        &binary,
        &invocation_args,
        cmd.ephemeral,
        vizier_root_existed_before_runtime,
    )?;

    if repeat == 1 {
        let summary = summaries.first().ok_or("missing run summary")?;
        if !cmd.follow {
            emit_enqueue_summary(
                cmd.format,
                &source,
                &summary.enqueue,
                &summary.root_jobs,
                cmd.ephemeral,
            )?;
            return Ok(());
        }

        let terminal = follow_run(
            project_root,
            jobs_root,
            &binary,
            &summary.run_id,
            &summary.job_ids,
            cmd.ephemeral,
            cmd.format,
        )?;
        emit_follow_summary(
            cmd.format,
            &source,
            &summary.enqueue,
            &summary.root_jobs,
            cmd.ephemeral,
            &terminal,
        )?;

        if terminal.exit_code == 0 {
            return Ok(());
        }
        std::process::exit(terminal.exit_code);
    }

    let mode = MultiRunMode::Repeat { repeat };

    if !cmd.follow {
        emit_multi_enqueue_summary(cmd.format, &source, &mode, &summaries, cmd.ephemeral)?;
        return Ok(());
    }

    let (followed_runs, terminal_state, exit_code) = follow_serial_runs(
        project_root,
        jobs_root,
        &binary,
        &summaries,
        cmd.ephemeral,
        cmd.format,
    )?;
    emit_multi_follow_summary(
        cmd.format,
        &source,
        &mode,
        &summaries,
        &followed_runs,
        cmd.ephemeral,
        &terminal_state,
        exit_code,
    )?;

    if exit_code == 0 {
        Ok(())
    } else {
        std::process::exit(exit_code)
    }
}

#[allow(clippy::too_many_arguments)]
fn enqueue_serial_runs(
    project_root: &Path,
    jobs_root: &Path,
    source: &ResolvedWorkflowSource,
    items: &[PreparedRunItem],
    requested_after: &[String],
    approval_override: Option<bool>,
    binary: &Path,
    invocation_args: &[String],
    ephemeral: bool,
    vizier_root_existed_before_runtime: bool,
) -> Result<Vec<EnqueuedRunSummary>, Box<dyn std::error::Error>> {
    let mut summaries = Vec::<EnqueuedRunSummary>::with_capacity(items.len());
    let mut previous_run_id = None::<String>;

    for item in items {
        let run_id = format!("run_{}", Uuid::new_v4().simple());
        let enqueue = jobs::enqueue_workflow_run_with_options(
            project_root,
            jobs_root,
            &run_id,
            &source.selector,
            &item.template,
            invocation_args,
            None,
            jobs::WorkflowRunEnqueueOptions {
                ephemeral,
                vizier_root_existed_before_runtime: ephemeral
                    .then_some(vizier_root_existed_before_runtime),
            },
        )?;

        let mut job_ids = enqueue.job_ids.values().cloned().collect::<Vec<_>>();
        job_ids.sort();
        let mut root_jobs = resolve_root_jobs(jobs_root, &job_ids)?;

        if let Some(alias) = source.command_alias.as_ref() {
            annotate_alias_metadata(jobs_root, &job_ids, alias.as_str())?;
        }

        let mut current_after = requested_after.to_vec();
        if let Some(previous) = previous_run_id.as_ref() {
            current_after.push(format!("{RUN_AFTER_PREFIX}{previous}"));
        }
        let normalized_after = normalize_after_dependencies(jobs_root, &current_after)?;
        if !normalized_after.is_empty() {
            for root in &root_jobs {
                let dependencies = jobs::resolve_after_dependencies_for_enqueue(
                    jobs_root,
                    root,
                    &normalized_after,
                )?;
                apply_after_dependencies(jobs_root, root, &dependencies)?;
            }
        }

        if let Some(required) = approval_override {
            for root in &root_jobs {
                apply_approval_override(jobs_root, root, required)?;
            }
        }

        // Keep deterministic startup by applying per-run root overrides before ticking.
        let _ = jobs::scheduler_tick(project_root, jobs_root, binary)?;

        root_jobs.sort();
        summaries.push(EnqueuedRunSummary {
            index: item.index,
            run_id: run_id.clone(),
            enqueue,
            job_ids,
            root_jobs,
            batch: item.batch.clone(),
        });
        previous_run_id = Some(run_id);
    }

    Ok(summaries)
}

fn follow_serial_runs(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    summaries: &[EnqueuedRunSummary],
    ephemeral: bool,
    format: RunFormatArg,
) -> Result<(Vec<FollowedRunSummary>, String, i32), Box<dyn std::error::Error>> {
    let mut followed_runs = Vec::<FollowedRunSummary>::new();
    for summary in summaries {
        let terminal = follow_run(
            project_root,
            jobs_root,
            binary,
            &summary.run_id,
            &summary.job_ids,
            ephemeral,
            format,
        )?;
        let should_stop = terminal.exit_code != 0;
        followed_runs.push(FollowedRunSummary {
            index: summary.index,
            run_id: summary.run_id.clone(),
            terminal,
            batch: summary.batch.clone(),
        });
        if should_stop {
            break;
        }
    }

    let (terminal_state, exit_code) = followed_runs
        .last()
        .map(|entry| {
            (
                entry.terminal.terminal_state.clone(),
                entry.terminal.exit_code,
            )
        })
        .unwrap_or_else(|| ("succeeded".to_string(), 0));

    Ok((followed_runs, terminal_state, exit_code))
}

fn prepare_batch_run(
    project_root: &Path,
    spec_dir: &str,
    prepared: &PreparedWorkflowInvocation,
) -> Result<PreparedBatchRun, Box<dyn std::error::Error>> {
    validate_batch_invocation(prepared)?;

    let (canonical_root, batch_root, batch_dir) = resolve_batch_dir(project_root, spec_dir)?;
    let discovered = discover_batch_specs(&canonical_root, &batch_root, &batch_dir)?;

    let mut items = Vec::<PreparedRunItem>::with_capacity(discovered.len());
    let mut slug_to_path = HashMap::<String, String>::new();
    for (index, spec) in discovered.into_iter().enumerate() {
        let slug = derive_batch_slug(&batch_root, &spec.absolute_path)?;
        if let Some(existing) = slug_to_path.insert(slug.clone(), spec.spec_file.clone()) {
            return Err(format!(
                "batch slug collision `{slug}` from `{existing}` and `{}`",
                spec.spec_file
            )
            .into());
        }

        let mut set_overrides = prepared.set_overrides.clone();
        set_overrides.insert("spec_file".to_string(), spec.spec_file.clone());
        set_overrides.insert("slug".to_string(), slug.clone());
        set_overrides.insert(
            "branch".to_string(),
            vizier_core::plan::default_branch_for_slug(&slug),
        );

        let item_index = (index + 1) as u32;
        let template = prepare_workflow_template_from_invocation(
            project_root,
            &prepared.source,
            &prepared.input_spec,
            &set_overrides,
        )
        .map_err(|err| {
            format!(
                "batch item #{} `{}` failed queue-time preparation: {err}",
                item_index, spec.spec_file
            )
        })?;
        jobs::validate_workflow_run_template(&template).map_err(|err| {
            format!(
                "batch item #{} `{}` failed validation: {err}",
                item_index, spec.spec_file
            )
        })?;

        items.push(PreparedRunItem {
            index: item_index,
            template,
            batch: Some(BatchRunItemMetadata {
                spec_file: spec.spec_file,
                slug,
            }),
        });
    }

    Ok(PreparedBatchRun { batch_dir, items })
}

fn validate_batch_invocation(
    prepared: &PreparedWorkflowInvocation,
) -> Result<(), Box<dyn std::error::Error>> {
    if !prepared
        .input_spec
        .params
        .iter()
        .any(|param| param == "spec_file")
    {
        return Err(format!(
            "workflow `{}` does not declare `spec_file`; `--spec-dir` requires a `spec_file` input",
            prepared.source.selector
        )
        .into());
    }

    if prepared
        .set_overrides
        .get("spec_file")
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err("`--spec-dir` cannot be combined with explicit `spec_file` input".into());
    }
    if prepared
        .set_overrides
        .get("slug")
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(
            "`--spec-dir` owns per-item slug assignment and cannot be combined with explicit `slug` or `name` overrides"
                .into(),
        );
    }
    if prepared
        .set_overrides
        .get("branch")
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(
            "`--spec-dir` cannot be combined with explicit `branch` or `source` overrides because batch items must use distinct branches"
                .into(),
        );
    }

    Ok(())
}

fn resolve_batch_dir(
    project_root: &Path,
    spec_dir: &str,
) -> Result<(PathBuf, PathBuf, String), Box<dyn std::error::Error>> {
    let canonical_root =
        fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let candidate = if Path::new(spec_dir).is_absolute() {
        PathBuf::from(spec_dir)
    } else {
        project_root.join(spec_dir)
    };
    let canonical_dir = fs::canonicalize(&candidate)
        .map_err(|err| format!("invalid --spec-dir `{spec_dir}`: {err}"))?;
    let metadata = fs::metadata(&canonical_dir)
        .map_err(|err| format!("invalid --spec-dir `{spec_dir}`: {err}"))?;
    if !metadata.is_dir() {
        return Err(format!("invalid --spec-dir `{spec_dir}`: path is not a directory").into());
    }
    if !canonical_dir.starts_with(&canonical_root) {
        return Err(format!(
            "invalid --spec-dir `{spec_dir}`: directory must stay under repo root `{}`",
            canonical_root.display()
        )
        .into());
    }

    let batch_dir = relative_display_path(&canonical_root, &canonical_dir);
    Ok((canonical_root, canonical_dir, batch_dir))
}

fn discover_batch_specs(
    canonical_root: &Path,
    batch_root: &Path,
    batch_dir: &str,
) -> Result<Vec<DiscoveredBatchSpec>, Box<dyn std::error::Error>> {
    let mut discovered = Vec::<DiscoveredBatchSpec>::new();
    collect_batch_specs_recursive(canonical_root, batch_dir, batch_root, &mut discovered)?;
    discovered.sort_by(|left, right| left.spec_file.as_bytes().cmp(right.spec_file.as_bytes()));

    if discovered.is_empty() {
        return Err(
            format!("invalid --spec-dir `{batch_dir}`: no markdown spec files found").into(),
        );
    }

    Ok(discovered)
}

fn collect_batch_specs_recursive(
    canonical_root: &Path,
    batch_dir: &str,
    dir: &Path,
    out: &mut Vec<DiscoveredBatchSpec>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| {
        relative_display_path(canonical_root, &left.path())
            .as_bytes()
            .cmp(relative_display_path(canonical_root, &right.path()).as_bytes())
    });

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            let symlink_path = relative_display_path(canonical_root, &path);
            return Err(format!(
                "invalid --spec-dir `{batch_dir}`: symlink entry `{symlink_path}` is unsupported; `--spec-dir` requires regular directories and markdown files"
            )
            .into());
        }

        if file_type.is_dir() {
            collect_batch_specs_recursive(canonical_root, batch_dir, &path, out)?;
            continue;
        }

        if file_type.is_file() && is_markdown_path(&path) {
            out.push(DiscoveredBatchSpec {
                spec_file: relative_display_path(canonical_root, &path),
                absolute_path: path,
            });
        }
    }

    Ok(())
}

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("md"))
}

fn derive_batch_slug(
    batch_root: &Path,
    spec_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let relative = spec_path.strip_prefix(batch_root).map_err(|err| {
        format!(
            "batch spec path `{}` is outside batch root `{}`: {err}",
            spec_path.display(),
            batch_root.display()
        )
    })?;
    let stem = relative.with_extension("");
    let candidate = normalize_path_for_output(&stem).replace('/', "-");
    vizier_core::plan::sanitize_name_override(&candidate).map_err(|err| {
        format!(
            "batch spec `{}` resolves to invalid slug `{candidate}`: {err}",
            normalize_path_for_output(relative)
        )
        .into()
    })
}

fn relative_display_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    normalize_path_for_output(relative)
}

fn normalize_path_for_output(path: &Path) -> String {
    let rendered = path.display().to_string().replace('\\', "/");
    if rendered.is_empty() {
        ".".to_string()
    } else {
        rendered
    }
}

fn normalize_after_dependencies(
    jobs_root: &Path,
    requested_after: &[String],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();

    for raw in requested_after {
        match parse_after_reference(raw)? {
            AfterReference::JobId(job_id) => {
                if seen.insert(job_id.clone()) {
                    deduped.push(job_id);
                }
            }
            AfterReference::RunId(run_id) => {
                let expanded = expand_run_after_reference(jobs_root, &run_id)?;
                for job_id in expanded {
                    if seen.insert(job_id.clone()) {
                        deduped.push(job_id);
                    }
                }
            }
        }
    }

    Ok(deduped)
}

fn parse_after_reference(raw: &str) -> Result<AfterReference, Box<dyn std::error::Error>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("unknown --after job id: <empty>".into());
    }

    if let Some(run_id) = trimmed.strip_prefix(RUN_AFTER_PREFIX) {
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Err(
                "invalid --after run reference `run:`; expected `run:<run_id>`"
                    .to_string()
                    .into(),
            );
        }
        return Ok(AfterReference::RunId(run_id.to_string()));
    }

    if trimmed.starts_with(RUN_ID_PREFIX) {
        return Err(format!(
            "invalid --after reference `{trimmed}`; use `run:{trimmed}` for run dependencies"
        )
        .into());
    }

    Ok(AfterReference::JobId(trimmed.to_string()))
}

fn expand_run_after_reference(
    jobs_root: &Path,
    run_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let manifest_path = jobs_root.join("runs").join(format!("{run_id}.json"));
    let bytes = fs::read(&manifest_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            format!(
                "invalid --after run reference `run:{run_id}`: manifest not found at `{}`",
                manifest_path.display()
            )
        } else {
            format!(
                "invalid --after run reference `run:{run_id}`: unable to read manifest `{}`: {err}",
                manifest_path.display()
            )
        }
    })?;
    let manifest = serde_json::from_slice::<AfterDependencyRunManifest>(&bytes).map_err(|err| {
        format!(
            "invalid --after run reference `run:{run_id}`: unable to parse manifest `{}`: {err}",
            manifest_path.display()
        )
    })?;

    let mut sink_job_ids = Vec::new();
    let mut sink_job_to_node = HashMap::<String, String>::new();
    for (node_id, node) in &manifest.nodes {
        if !node.routes.succeeded.is_empty() {
            continue;
        }

        let sink_job_id = node.job_id.trim();
        if sink_job_id.is_empty() {
            return Err(format!(
                "invalid --after run reference `run:{run_id}`: sink node `{node_id}` has an empty job_id"
            )
            .into());
        }

        if let Some(existing_node_id) =
            sink_job_to_node.insert(sink_job_id.to_string(), node_id.clone())
        {
            return Err(format!(
                "invalid --after run reference `run:{run_id}`: duplicate sink job_id `{sink_job_id}` across nodes `{existing_node_id}` and `{node_id}`"
            )
            .into());
        }

        sink_job_ids.push(sink_job_id.to_string());
    }

    if sink_job_ids.is_empty() {
        return Err(format!(
            "invalid --after run reference `run:{run_id}`: manifest has no success-terminal sink nodes"
        )
        .into());
    }

    Ok(sink_job_ids)
}

fn resolve_root_jobs(
    jobs_root: &Path,
    job_ids: &[String],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut roots = Vec::new();
    for job_id in job_ids {
        let record = jobs::read_record(jobs_root, job_id)?;
        let schedule = record.schedule.unwrap_or_default();
        if schedule.after.is_empty() {
            roots.push(job_id.clone());
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn annotate_alias_metadata(
    jobs_root: &Path,
    job_ids: &[String],
    alias: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for job_id in job_ids {
        jobs::update_job_record(jobs_root, job_id, |record| {
            let metadata = record.metadata.get_or_insert_with(Default::default);
            metadata.command_alias = Some(alias.to_string());
        })?;
    }
    Ok(())
}

fn apply_after_dependencies(
    jobs_root: &Path,
    job_id: &str,
    dependencies: &[jobs::JobAfterDependency],
) -> Result<(), Box<dyn std::error::Error>> {
    jobs::update_job_record(jobs_root, job_id, |record| {
        let schedule = record.schedule.get_or_insert_with(Default::default);
        for dependency in dependencies {
            if schedule.after.iter().any(|existing| {
                existing.job_id == dependency.job_id && existing.policy == dependency.policy
            }) {
                continue;
            }
            schedule.after.push(dependency.clone());
        }
    })?;
    Ok(())
}

fn apply_approval_override(
    jobs_root: &Path,
    job_id: &str,
    required: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    jobs::update_job_record(jobs_root, job_id, |record| {
        let schedule = record.schedule.get_or_insert_with(Default::default);
        if required {
            schedule.approval = Some(jobs::pending_job_approval());
        } else {
            schedule.approval = None;
        }
    })?;
    Ok(())
}

fn emit_enqueue_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    enqueue: &jobs::EnqueueWorkflowRunResult,
    root_jobs: &[String],
    ephemeral: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let payload = json!({
            "outcome": "workflow_run_enqueued",
            "run_id": enqueue.run_id,
            "ephemeral": ephemeral,
            "workflow_template_selector": source.selector,
            "workflow_template_id": enqueue.template_id,
            "workflow_template_version": enqueue.template_version,
            "root_job_ids": root_jobs,
            "next": {
                "schedule": "vizier jobs schedule",
                "show": "vizier jobs show <job-id>",
                "tail": "vizier jobs tail <job-id> --follow"
            }
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let next_hint = if let Some(root) = root_jobs.first() {
        format!(
            "vizier jobs schedule --job {root}\nvizier jobs show {root}\nvizier jobs tail {root} --follow"
        )
    } else {
        "vizier jobs schedule".to_string()
    };

    println!(
        "{}",
        format_block(vec![
            ("Outcome".to_string(), "Workflow run enqueued".to_string()),
            ("Run".to_string(), enqueue.run_id.clone()),
            (
                "Template".to_string(),
                format!("{}@{}", enqueue.template_id, enqueue.template_version),
            ),
            ("Selector".to_string(), source.selector.clone(),),
            (
                "Root jobs".to_string(),
                if root_jobs.is_empty() {
                    "none".to_string()
                } else {
                    root_jobs.join(", ")
                },
            ),
            (
                "Ephemeral".to_string(),
                if ephemeral { "yes" } else { "no" }.to_string(),
            ),
            ("Next".to_string(), next_hint),
        ])
    );

    Ok(())
}

fn emit_validation_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    template: &vizier_core::workflow_template::WorkflowTemplate,
    batch: Option<&PreparedBatchRun>,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let mut payload = serde_json::Map::from_iter([
            ("outcome".to_string(), json!("workflow_validation_passed")),
            (
                "workflow_template_selector".to_string(),
                json!(source.selector),
            ),
            ("workflow_template_id".to_string(), json!(&template.id)),
            (
                "workflow_template_version".to_string(),
                json!(&template.version),
            ),
            ("node_count".to_string(), json!(template.nodes.len())),
        ]);
        if let Some(batch) = batch {
            payload.insert("batch_dir".to_string(), json!(&batch.batch_dir));
            payload.insert("spec_count".to_string(), json!(batch.items.len()));
            payload.insert(
                "items".to_string(),
                serde_json::Value::Array(
                    batch
                        .items
                        .iter()
                        .filter_map(|item| {
                            item.batch.as_ref().map(|batch| {
                                json!({
                                    "index": item.index,
                                    "spec_file": &batch.spec_file,
                                    "slug": &batch.slug,
                                })
                            })
                        })
                        .collect(),
                ),
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(payload))?
        );
        return Ok(());
    }

    let mut rows = vec![
        (
            "Outcome".to_string(),
            "Workflow validation passed".to_string(),
        ),
        ("Selector".to_string(), source.selector.clone()),
        (
            "Template".to_string(),
            format!("{}@{}", template.id, template.version),
        ),
        ("Nodes".to_string(), template.nodes.len().to_string()),
    ];
    if let Some(batch) = batch {
        let items = batch
            .items
            .iter()
            .filter_map(|item| {
                item.batch
                    .as_ref()
                    .map(|batch| format!("#{} {} => {}", item.index, batch.spec_file, batch.slug))
            })
            .collect::<Vec<_>>()
            .join("\n");
        rows.push(("Batch dir".to_string(), batch.batch_dir.clone()));
        rows.push(("Specs".to_string(), batch.items.len().to_string()));
        rows.push(("Items".to_string(), items));
    }

    println!("{}", format_block(rows));

    Ok(())
}

fn emit_multi_enqueue_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    mode: &MultiRunMode,
    summaries: &[EnqueuedRunSummary],
    ephemeral: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let runs = summaries
            .iter()
            .map(|summary| {
                json!({
                    "index": summary.index,
                    "run_id": &summary.run_id,
                    "workflow_template_id": &summary.enqueue.template_id,
                    "workflow_template_version": &summary.enqueue.template_version,
                    "root_job_ids": &summary.root_jobs,
                    "spec_file": summary.batch.as_ref().map(|batch| batch.spec_file.as_str()),
                    "slug": summary.batch.as_ref().map(|batch| batch.slug.as_str()),
                })
            })
            .collect::<Vec<_>>();
        let mut payload = serde_json::Map::from_iter([
            ("outcome".to_string(), json!("workflow_runs_enqueued")),
            ("ephemeral".to_string(), json!(ephemeral)),
            (
                "workflow_template_selector".to_string(),
                json!(source.selector),
            ),
            ("runs".to_string(), serde_json::Value::Array(runs)),
            (
                "next".to_string(),
                json!({
                    "schedule": "vizier jobs schedule",
                    "show": "vizier jobs show <job-id>",
                    "tail": "vizier jobs tail <job-id> --follow"
                }),
            ),
        ]);
        match mode {
            MultiRunMode::Repeat { repeat } => {
                payload.insert("repeat".to_string(), json!(repeat));
            }
            MultiRunMode::Batch {
                batch_dir,
                spec_count,
            } => {
                payload.insert("batch_dir".to_string(), json!(batch_dir));
                payload.insert("spec_count".to_string(), json!(spec_count));
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(payload))?
        );
        return Ok(());
    }

    let run_ids = summaries
        .iter()
        .map(|summary| summary.run_id.clone())
        .collect::<Vec<_>>();
    let root_map = summaries
        .iter()
        .map(|summary| {
            let roots = if summary.root_jobs.is_empty() {
                "none".to_string()
            } else {
                summary.root_jobs.join(", ")
            };
            format!("{}: {roots}", summary.run_id)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let template = summaries
        .first()
        .map(|summary| {
            format!(
                "{}@{}",
                summary.enqueue.template_id, summary.enqueue.template_version
            )
        })
        .unwrap_or_else(|| "unknown".to_string());
    let next_hint = summaries
        .first()
        .and_then(|summary| summary.root_jobs.first())
        .map(|root| {
            format!(
                "vizier jobs schedule --job {root}\nvizier jobs show {root}\nvizier jobs tail {root} --follow"
            )
        })
        .unwrap_or_else(|| "vizier jobs schedule".to_string());

    let mut rows = vec![
        ("Outcome".to_string(), "Workflow runs enqueued".to_string()),
        ("Runs".to_string(), run_ids.join(", ")),
        ("Template".to_string(), template),
        ("Selector".to_string(), source.selector.clone()),
        (
            "Ephemeral".to_string(),
            if ephemeral { "yes" } else { "no" }.to_string(),
        ),
    ];
    match mode {
        MultiRunMode::Repeat { repeat } => {
            rows.insert(1, ("Repeat".to_string(), repeat.to_string()));
            rows.push(("Root jobs".to_string(), root_map));
        }
        MultiRunMode::Batch {
            batch_dir,
            spec_count,
        } => {
            let items = summaries
                .iter()
                .filter_map(|summary| {
                    summary.batch.as_ref().map(|batch| {
                        let roots = if summary.root_jobs.is_empty() {
                            "none".to_string()
                        } else {
                            summary.root_jobs.join(", ")
                        };
                        format!(
                            "#{} {} slug={} run={} roots={roots}",
                            summary.index, batch.spec_file, batch.slug, summary.run_id
                        )
                    })
                })
                .collect::<Vec<_>>()
                .join("\n");
            rows.insert(1, ("Batch dir".to_string(), batch_dir.clone()));
            rows.insert(2, ("Specs".to_string(), spec_count.to_string()));
            rows.push(("Items".to_string(), items));
        }
    }
    rows.push(("Next".to_string(), next_hint));

    println!("{}", format_block(rows));

    Ok(())
}

#[derive(Debug, Clone)]
struct FollowResult {
    exit_code: i32,
    terminal_state: String,
    succeeded: Vec<String>,
    failed: Vec<String>,
    blocked: Vec<String>,
    cancelled: Vec<String>,
    cleanup: Option<jobs::EphemeralRunCleanupEvent>,
}

fn follow_run(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    run_id: &str,
    job_ids: &[String],
    ephemeral: bool,
    format: RunFormatArg,
) -> Result<FollowResult, Box<dyn std::error::Error>> {
    let stream_logs = matches!(format, RunFormatArg::Text);
    let mut last_status = HashMap::<String, jobs::JobStatus>::new();
    let mut last_log_line = HashMap::<String, String>::new();

    loop {
        let _ = jobs::scheduler_tick_without_ephemeral_cleanup(project_root, jobs_root, binary)?;

        let mut succeeded = Vec::new();
        let mut failed = Vec::new();
        let mut blocked = Vec::new();
        let mut cancelled = Vec::new();

        for job_id in job_ids {
            let record = jobs::read_record(jobs_root, job_id)?;
            let status = record.status;

            if stream_logs {
                if last_status.get(job_id) != Some(&status) {
                    println!("[run:{run_id}] {job_id} => {}", jobs::status_label(status));
                    last_status.insert(job_id.clone(), status);
                }
                if let Some(line) = jobs::latest_job_log_line(jobs_root, job_id, 2048)? {
                    let marker = format!("{}:{}", line.stream.label(), line.line);
                    if last_log_line.get(job_id) != Some(&marker) {
                        println!("[{job_id}/{}] {}", line.stream.label(), line.line);
                        last_log_line.insert(job_id.clone(), marker);
                    }
                }
            }

            match status {
                jobs::JobStatus::Succeeded => succeeded.push(job_id.clone()),
                jobs::JobStatus::Failed => failed.push(job_id.clone()),
                jobs::JobStatus::Cancelled => cancelled.push(job_id.clone()),
                jobs::JobStatus::BlockedByDependency | jobs::JobStatus::BlockedByApproval => {
                    blocked.push(job_id.clone())
                }
                jobs::JobStatus::Queued
                | jobs::JobStatus::WaitingOnDeps
                | jobs::JobStatus::WaitingOnApproval
                | jobs::JobStatus::WaitingOnLocks
                | jobs::JobStatus::Running => {}
            }
        }

        let terminal_count = succeeded.len() + failed.len() + blocked.len() + cancelled.len();
        if terminal_count == job_ids.len() {
            succeeded.sort();
            failed.sort();
            blocked.sort();
            cancelled.sort();

            let (terminal_state, exit_code) = if !failed.is_empty() || !cancelled.is_empty() {
                ("failed".to_string(), 1)
            } else if !blocked.is_empty() {
                ("blocked".to_string(), 10)
            } else {
                ("succeeded".to_string(), 0)
            };
            let cleanup = if ephemeral {
                let mut cleanup = jobs::scheduler_tick(project_root, jobs_root, binary)?
                    .ephemeral_run_cleanups
                    .into_iter()
                    .find(|entry| entry.run_id == run_id);
                if cleanup.is_none() {
                    cleanup = jobs::scheduler_tick(project_root, jobs_root, binary)?
                        .ephemeral_run_cleanups
                        .into_iter()
                        .find(|entry| entry.run_id == run_id);
                }
                cleanup
            } else {
                None
            };

            return Ok(FollowResult {
                exit_code,
                terminal_state,
                succeeded,
                failed,
                blocked,
                cancelled,
                cleanup,
            });
        }

        thread::sleep(Duration::from_millis(120));
    }
}

fn emit_follow_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    enqueue: &jobs::EnqueueWorkflowRunResult,
    root_jobs: &[String],
    ephemeral: bool,
    result: &FollowResult,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let payload = json!({
            "outcome": "workflow_run_terminal",
            "terminal_state": result.terminal_state,
            "exit_code": result.exit_code,
            "run_id": enqueue.run_id,
            "ephemeral": ephemeral,
            "workflow_template_selector": source.selector,
            "workflow_template_id": enqueue.template_id,
            "workflow_template_version": enqueue.template_version,
            "root_job_ids": root_jobs,
            "succeeded": result.succeeded,
            "failed": result.failed,
            "blocked": result.blocked,
            "cancelled": result.cancelled,
            "ephemeral_cleanup": result.cleanup,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let outcome = match result.terminal_state.as_str() {
        "succeeded" => "Workflow run succeeded",
        "blocked" => "Workflow run blocked",
        _ => "Workflow run failed",
    };

    let mut rows = vec![
        ("Outcome".to_string(), outcome.to_string()),
        ("Run".to_string(), enqueue.run_id.clone()),
        (
            "Template".to_string(),
            format!("{}@{}", enqueue.template_id, enqueue.template_version),
        ),
        ("Selector".to_string(), source.selector.clone()),
        (
            "Root jobs".to_string(),
            if root_jobs.is_empty() {
                "none".to_string()
            } else {
                root_jobs.join(", ")
            },
        ),
        (
            "Ephemeral".to_string(),
            if ephemeral { "yes" } else { "no" }.to_string(),
        ),
        ("Exit".to_string(), result.exit_code.to_string()),
    ];

    if !result.succeeded.is_empty() {
        rows.push(("Succeeded".to_string(), result.succeeded.join(", ")));
    }
    if !result.blocked.is_empty() {
        rows.push(("Blocked".to_string(), result.blocked.join(", ")));
    }
    if !result.failed.is_empty() {
        rows.push(("Failed".to_string(), result.failed.join(", ")));
    }
    if !result.cancelled.is_empty() {
        rows.push(("Cancelled".to_string(), result.cancelled.join(", ")));
    }
    if let Some(cleanup) = result.cleanup.as_ref() {
        rows.push((
            "Ephemeral cleanup".to_string(),
            cleanup.state.label().to_string(),
        ));
        if let Some(detail) = cleanup.detail.as_ref() {
            rows.push(("Cleanup detail".to_string(), detail.clone()));
        }
        if !cleanup.degraded_notes.is_empty() {
            rows.push((
                "Cleanup notes".to_string(),
                cleanup.degraded_notes.join("\n"),
            ));
        }
    }

    println!("{}", format_block(rows));
    if result.exit_code == 10 {
        display::warn("run reached a blocked terminal state");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_multi_follow_summary(
    format: RunFormatArg,
    source: &ResolvedWorkflowSource,
    mode: &MultiRunMode,
    summaries: &[EnqueuedRunSummary],
    followed_runs: &[FollowedRunSummary],
    ephemeral: bool,
    terminal_state: &str,
    exit_code: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(format, RunFormatArg::Json) {
        let runs = followed_runs
            .iter()
            .map(|entry| {
                json!({
                    "index": entry.index,
                    "run_id": &entry.run_id,
                    "terminal_state": &entry.terminal.terminal_state,
                    "exit_code": entry.terminal.exit_code,
                    "succeeded": &entry.terminal.succeeded,
                    "failed": &entry.terminal.failed,
                    "blocked": &entry.terminal.blocked,
                    "cancelled": &entry.terminal.cancelled,
                    "ephemeral_cleanup": &entry.terminal.cleanup,
                    "spec_file": entry.batch.as_ref().map(|batch| batch.spec_file.as_str()),
                    "slug": entry.batch.as_ref().map(|batch| batch.slug.as_str()),
                })
            })
            .collect::<Vec<_>>();
        let mut payload = serde_json::Map::from_iter([
            ("outcome".to_string(), json!("workflow_runs_terminal")),
            ("ephemeral".to_string(), json!(ephemeral)),
            ("terminal_state".to_string(), json!(terminal_state)),
            ("exit_code".to_string(), json!(exit_code)),
            (
                "workflow_template_selector".to_string(),
                json!(source.selector),
            ),
            ("runs".to_string(), serde_json::Value::Array(runs)),
        ]);
        match mode {
            MultiRunMode::Repeat { repeat } => {
                payload.insert("repeat".to_string(), json!(repeat));
            }
            MultiRunMode::Batch {
                batch_dir,
                spec_count,
            } => {
                payload.insert("batch_dir".to_string(), json!(batch_dir));
                payload.insert("spec_count".to_string(), json!(spec_count));
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(payload))?
        );
        return Ok(());
    }

    let outcome = match terminal_state {
        "succeeded" => "Workflow runs succeeded",
        "blocked" => "Workflow runs blocked",
        _ => "Workflow runs failed",
    };
    let all_runs = summaries
        .iter()
        .map(|summary| summary.run_id.clone())
        .collect::<Vec<_>>();
    let followed = followed_runs
        .iter()
        .map(|entry| entry.run_id.clone())
        .collect::<Vec<_>>();
    let run_states = followed_runs
        .iter()
        .map(|entry| {
            if let Some(batch) = entry.batch.as_ref() {
                format!(
                    "#{} {} ({}) => {} {} ({})",
                    entry.index,
                    batch.spec_file,
                    batch.slug,
                    entry.run_id,
                    entry.terminal.terminal_state,
                    entry.terminal.exit_code
                )
            } else {
                format!(
                    "#{} {} => {} ({})",
                    entry.index,
                    entry.run_id,
                    entry.terminal.terminal_state,
                    entry.terminal.exit_code
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let template = summaries
        .first()
        .map(|summary| {
            format!(
                "{}@{}",
                summary.enqueue.template_id, summary.enqueue.template_version
            )
        })
        .unwrap_or_else(|| "unknown".to_string());

    let mut rows = vec![
        ("Outcome".to_string(), outcome.to_string()),
        ("Runs".to_string(), all_runs.join(", ")),
        (
            "Followed".to_string(),
            if followed.is_empty() {
                "none".to_string()
            } else {
                followed.join(", ")
            },
        ),
        ("Template".to_string(), template),
        ("Selector".to_string(), source.selector.clone()),
        (
            "Ephemeral".to_string(),
            if ephemeral { "yes" } else { "no" }.to_string(),
        ),
        ("Exit".to_string(), exit_code.to_string()),
    ];
    match mode {
        MultiRunMode::Repeat { repeat } => {
            rows.insert(1, ("Repeat".to_string(), repeat.to_string()));
        }
        MultiRunMode::Batch {
            batch_dir,
            spec_count,
        } => {
            rows.insert(1, ("Batch dir".to_string(), batch_dir.clone()));
            rows.insert(2, ("Specs".to_string(), spec_count.to_string()));
        }
    }
    if !run_states.is_empty() {
        rows.push(("Run states".to_string(), run_states));
    }

    println!("{}", format_block(rows));
    if exit_code == 10 {
        display::warn("run reached a blocked terminal state");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_after_reference_rejects_bare_run_id() {
        let err = parse_after_reference("run_deadbeef").expect_err("expected run-id guidance");
        assert!(
            err.to_string().contains("use `run:run_deadbeef`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn normalize_after_dependencies_expands_run_sinks_and_dedupes() {
        let temp = TempDir::new().expect("temp dir");
        let runs_dir = temp.path().join("runs");
        std::fs::create_dir_all(&runs_dir).expect("create runs dir");
        std::fs::write(
            runs_dir.join("run_prev.json"),
            r#"{
  "nodes": {
    "node_a": {
      "job_id": "job-a",
      "routes": { "succeeded": [] }
    },
    "node_b": {
      "job_id": "job-b",
      "routes": { "succeeded": [] }
    },
    "node_c": {
      "job_id": "job-c",
      "routes": { "succeeded": [{ "node_id": "node_d", "mode": "propagate_context" }] }
    }
  }
}"#,
        )
        .expect("write run manifest");

        let dependencies = normalize_after_dependencies(
            temp.path(),
            &[
                "run:run_prev".to_string(),
                "manual-job".to_string(),
                "job-a".to_string(),
                "run:run_prev".to_string(),
            ],
        )
        .expect("resolve dependencies");
        assert_eq!(dependencies, vec!["job-a", "job-b", "manual-job"]);
    }

    #[test]
    fn expand_run_after_reference_rejects_manifests_without_success_sinks() {
        let temp = TempDir::new().expect("temp dir");
        let runs_dir = temp.path().join("runs");
        std::fs::create_dir_all(&runs_dir).expect("create runs dir");
        std::fs::write(
            runs_dir.join("run_prev.json"),
            r#"{
  "nodes": {
    "node_only": {
      "job_id": "job-only",
      "routes": { "succeeded": [{ "node_id": "node_only", "mode": "propagate_context" }] }
    }
  }
}"#,
        )
        .expect("write run manifest");

        let err = expand_run_after_reference(temp.path(), "run_prev")
            .expect_err("expected zero-sink error");
        assert!(
            err.to_string().contains("no success-terminal sink nodes"),
            "unexpected error: {err}"
        );
    }
}
