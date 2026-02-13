use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Write};

use clap::{ColorChoice, CommandFactory, FromArgMatches, error::ErrorKind};
use vizier_core::{
    auditor, config,
    display::{self, LogLevel},
    tools, vcs,
    workflow_template::{
        WorkflowCapability, WorkflowGate, WorkflowNode, WorkflowPrecondition, WorkflowTemplate,
        workflow_node_capability,
    },
};

use crate::actions::*;
use crate::cli::args::*;
use crate::cli::help::{
    curated_help_text, pager_mode_from_args, render_clap_help_text,
    render_clap_subcommand_help_text, render_help_with_pager, strip_ansi_codes,
    subcommand_from_raw_args,
};
use crate::cli::jobs_view::run_jobs_command;
use crate::cli::prompt::{ReviewQueueChoice, prompt_review_queue_choice, prompt_yes_no};
use crate::cli::resolve::{
    build_cli_agent_overrides, resolve_approve_options, resolve_cd_options, resolve_clean_options,
    resolve_draft_spec, resolve_list_options, resolve_merge_options, resolve_review_options,
    resolve_test_display_options,
};
use crate::cli::scheduler::{
    background_config_snapshot, build_background_child_args, build_job_metadata,
    capture_save_input_patch, emit_job_summary, generate_job_id, has_active_plan_job,
    prepare_prompt_input, resolve_pinned_head, run_scheduled_save, runtime_job_metadata,
    scheduler_supported, user_friendly_args,
};
use crate::cli::util::flag_present;
use crate::workflow_templates::{
    MergeTemplateGateConfig, TemplateScope, WorkflowTemplateNodeSchedule, WorkflowTemplateRef,
    compile_template_node, compile_template_node_schedule, resolve_approve_template,
    resolve_custom_alias_template, resolve_draft_template, resolve_merge_template,
    resolve_patch_template, resolve_primary_template_node_id, resolve_review_template,
    resolve_save_template, resolve_template_ref, resolve_template_ref_for_alias,
    validate_template_agent_backends,
};
use crate::{jobs, plan};

pub(crate) async fn run() -> Result<(), Box<dyn std::error::Error>> {
    if crate::completions::try_handle_completion(Cli::command)
        .map_err(Box::<dyn std::error::Error>::from)?
    {
        return Ok(());
    }

    let stdout_is_tty = std::io::stdout().is_terminal();
    let stderr_is_tty = std::io::stderr().is_terminal();
    let raw_args: Vec<String> = std::env::args().collect();
    if matches!(subcommand_from_raw_args(&raw_args).as_deref(), Some("ask")) {
        return Err("`ask` has been removed; use supported workflow commands (`save`, `draft`, `approve`, `review`, `merge`, `run`).".into());
    }
    if let Some(message) = removed_global_flag_guidance(&raw_args) {
        return Err(message.into());
    }
    let quiet_requested = flag_present(&raw_args, Some('q'), "--quiet");
    let no_ansi_requested = flag_present(&raw_args, None, "--no-ansi");
    let pager_mode = pager_mode_from_args(&raw_args);

    let color_choice = if !no_ansi_requested && stdout_is_tty && stderr_is_tty {
        ColorChoice::Auto
    } else {
        ColorChoice::Never
    };

    let command = Cli::command().color(color_choice);
    let matches = match command.try_get_matches_from(&raw_args) {
        Ok(matches) => matches,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                let rendered = match err.kind() {
                    ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                        curated_help_text().to_string()
                    }
                    ErrorKind::DisplayHelp => {
                        if subcommand_from_raw_args(&raw_args).is_none() {
                            curated_help_text().to_string()
                        } else {
                            err.render().to_string()
                        }
                    }
                    _ => unreachable!("caller guards ensure help error kinds"),
                };

                let help_text = if color_choice != ColorChoice::Never {
                    rendered
                } else {
                    strip_ansi_codes(&rendered)
                };

                render_help_with_pager(&help_text, pager_mode, stdout_is_tty, quiet_requested)?;
                return Ok(());
            }
            ErrorKind::DisplayVersion => {
                let rendered = err.render().to_string();
                println!("{rendered}");
                return Ok(());
            }
            _ => err.exit(),
        },
    };

    let cli = Cli::from_arg_matches(&matches)?;
    jobs::set_current_job_id(cli.global.background_job_id.clone());

    let mut verbosity = if cli.global.quiet {
        display::Verbosity::Quiet
    } else {
        match cli.global.verbose {
            0 => display::Verbosity::Normal,
            1 => display::Verbosity::Info,
            _ => display::Verbosity::Debug,
        }
    };

    if !cli.global.quiet && cli.global.debug {
        verbosity = display::Verbosity::Debug;
    }

    display::set_display_config(display::DisplayConfig {
        verbosity,
        stdout_is_tty,
        stderr_is_tty,
    });

    if let Commands::Help(cmd) = &cli.command {
        let rendered = if cmd.all {
            render_clap_help_text(color_choice)
        } else if let Some(command) = cmd.command.as_deref() {
            render_clap_subcommand_help_text(color_choice, command)?
        } else {
            curated_help_text().to_string()
        };

        let help_text = if color_choice != ColorChoice::Never {
            rendered
        } else {
            strip_ansi_codes(&rendered)
        };

        render_help_with_pager(&help_text, pager_mode, stdout_is_tty, quiet_requested)?;
        return Ok(());
    }

    let project_root = match auditor::find_project_root() {
        Ok(Some(root)) => root,
        Ok(None) => {
            display::emit(
                LogLevel::Error,
                "vizier cannot be used outside a git repository",
            );
            return Err("not a git repository".into());
        }
        Err(e) => {
            display::emit(LogLevel::Error, format!("Error finding project root: {e}"));
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    if let Commands::Init(cmd) = &cli.command {
        return run_init(&project_root, cmd.check);
    }

    if let Err(e) = std::fs::create_dir_all(project_root.join(".vizier")) {
        display::emit(
            LogLevel::Error,
            format!("Error creating .vizier directory: {e}"),
        );
        return Err(Box::<dyn std::error::Error>::from(e));
    }

    if let Err(e) = std::fs::create_dir_all(project_root.join(".vizier").join("sessions")) {
        display::emit(
            LogLevel::Error,
            format!("Error creating .vizier/sessions directory: {e}"),
        );
        return Err(Box::<dyn std::error::Error>::from(e));
    }

    let jobs_root = match jobs::ensure_jobs_root(&project_root) {
        Ok(path) => path,
        Err(e) => {
            display::emit(
                LogLevel::Error,
                format!("Error creating .vizier/jobs directory: {e}"),
            );
            return Err(Box::<dyn std::error::Error>::from(e));
        }
    };

    let mut auditor_cleanup = auditor::AuditorCleanup {
        debug: cli.global.debug,
        print_json: false,
        persisted: false,
    };

    if let Err(e) = std::fs::create_dir_all(tools::get_vizier_dir()) {
        display::emit(
            LogLevel::Error,
            format!(
                "Error creating .vizier directory {:?}: {e}",
                tools::get_vizier_dir()
            ),
        );

        return Err(Box::<dyn std::error::Error>::from(e));
    }

    if let Err(e) = std::fs::create_dir_all(tools::get_narrative_dir()) {
        display::emit(
            LogLevel::Error,
            format!(
                "Error creating .vizier/narrative directory {:?}: {e}",
                tools::get_narrative_dir()
            ),
        );

        return Err(Box::<dyn std::error::Error>::from(e));
    }

    let mut cfg = if let Some(ref config_file) = cli.global.config_file {
        config::load_config_from_path(std::path::PathBuf::from(config_file))?
    } else {
        let mut layers = Vec::new();

        if let Some(path) = config::global_config_path().filter(|path| path.exists()) {
            display::emit(
                LogLevel::Info,
                format!("Loading global config from {}", path.display()),
            );
            layers.push(config::load_config_layer_from_path(path)?);
        }

        if let Some(path) = config::project_config_path(&project_root) {
            display::emit(
                LogLevel::Info,
                format!("Loading repo config from {}", path.display()),
            );
            layers.push(config::load_config_layer_from_path(path)?);
        }

        if !layers.is_empty() {
            config::Config::from_layers(&layers)
        } else if let Some(path) = config::env_config_path().filter(|path| path.exists()) {
            display::emit(
                LogLevel::Info,
                format!("Loading env config from {}", path.display()),
            );
            config::load_config_from_path(path)?
        } else {
            config::get_config()
        }
    };

    if let Some(session_id) = &cli.global.load_session {
        let repo_session = project_root
            .join(".vizier")
            .join("sessions")
            .join(session_id)
            .join("session.json");

        let messages = if repo_session.exists() {
            auditor::Auditor::load_session_messages_from_path(&repo_session)?
        } else {
            return Err("could not find session file".into());
        };

        auditor::Auditor::replace_messages(&messages);
    }

    cfg.no_session = cli.global.no_session;

    if let Some(selector) = cli
        .global
        .agent
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        cfg.agent_selector = selector.to_ascii_lowercase();
        cfg.backend = config::backend_kind_for_selector(&cfg.agent_selector);
    }

    if let Some(job_id) = cli.global.background_job_id.as_deref()
        && let Ok(record) = jobs::read_record(&jobs_root, job_id)
        && let Some(snapshot) = record.config_snapshot.as_ref()
    {
        apply_background_config_snapshot(&mut cfg, snapshot);
    }

    let workflow_defaults = cfg.workflow.clone();

    config::set_config(cfg);

    let cli_agent_override = build_cli_agent_overrides(&cli.global)?;

    let push_after = cli.global.push;
    let commit_mode = if cli.global.no_commit || workflow_defaults.no_commit_default {
        CommitMode::HoldForReview
    } else {
        CommitMode::AutoCommit
    };

    let follow = cli.global.follow;

    if cli.global.background_job_id.is_none() && scheduler_supported(&cli.command) {
        let job_id = generate_job_id();
        let config_snapshot = background_config_snapshot(&config::get_config());
        let passthrough_global_args = workflow_node_passthrough_global_args(&cli.global);
        let requested_after = match &cli.command {
            Commands::Save(cmd) => cmd.after.clone(),
            Commands::Draft(cmd) => cmd.after.clone(),
            Commands::Patch(cmd) => cmd.after.clone(),
            Commands::Approve(cmd) => cmd.after.clone(),
            Commands::Review(cmd) => cmd.after.clone(),
            Commands::Merge(cmd) => cmd.after.clone(),
            Commands::Run(cmd) => cmd.after.clone(),
            _ => Vec::new(),
        };

        let mut metadata = build_job_metadata(
            &cli.command,
            &config::get_config(),
            cli_agent_override.as_ref(),
        );

        match &cli.command {
            Commands::Save(cmd) => {
                if cmd.commit_message_editor {
                    return Err(
                        "--commit-message-editor is not supported for scheduled runs".into(),
                    );
                }
                let pinned = resolve_pinned_head(&project_root)?;
                let (_alias, template_scope, template_ref) = resolve_wrapper_template_ref(
                    &config::get_config(),
                    "save",
                    TemplateScope::Save,
                )?;
                let template = resolve_save_template(&template_ref, &pinned.branch, &job_id)?;
                let primary_node_id = resolve_primary_template_node_id(&template, template_scope)?;
                metadata.target = Some(pinned.branch.clone());
                let mut primary_node_arg_overrides = BTreeMap::new();
                primary_node_arg_overrides
                    .insert("rev_or_range".to_string(), cmd.rev_or_range.clone());
                if let Some(message) = cmd.commit_message.as_ref() {
                    primary_node_arg_overrides
                        .insert("commit_message".to_string(), message.clone());
                }
                primary_node_arg_overrides
                    .insert("commit_mode".to_string(), commit_mode.label().to_string());
                primary_node_arg_overrides.insert("push_after".to_string(), push_after.to_string());

                enqueue_wrapper_template_graph(EnqueueWrapperTemplateGraphRequest {
                    project_root: &project_root,
                    jobs_root: &jobs_root,
                    root_job_id: &job_id,
                    follow_primary: follow,
                    background: &workflow_defaults.background,
                    config_snapshot: &config_snapshot,
                    requested_after: &requested_after,
                    base_metadata: &metadata,
                    template_ref: &template_ref,
                    scope_label: template_scope.label().to_string(),
                    compat_scope: Some(template_scope),
                    template: &template,
                    primary_node_id: &primary_node_id,
                    pinned_head: Some(pinned),
                    capture_save_patch_for_primary: true,
                    primary_node_arg_overrides: &primary_node_arg_overrides,
                    passthrough_global_args: &passthrough_global_args,
                    cli_agent_override: cli_agent_override.as_ref(),
                })?;
            }
            Commands::Draft(cmd) => {
                let (resolved, input_file) = prepare_prompt_input(
                    cmd.spec.as_deref(),
                    cmd.file.as_deref(),
                    &project_root,
                    &job_id,
                )?;
                let plan_dir = project_root.join(".vizier/implementation-plans");
                std::fs::create_dir_all(&plan_dir)?;
                let base_slug = if let Some(name) = cmd.name.as_ref() {
                    plan::sanitize_name_override(name)?
                } else {
                    plan::slug_from_spec(&resolved.text)
                };
                let slug = plan::ensure_unique_slug(&base_slug, &plan_dir, "draft/")?;
                let branch = plan::default_branch_for_slug(&slug);
                let (_alias, template_scope, template_ref) = resolve_wrapper_template_ref(
                    &config::get_config(),
                    "draft",
                    TemplateScope::Draft,
                )?;
                let template = resolve_draft_template(&template_ref, &slug, &branch, &job_id)?;
                let primary_node_id = resolve_primary_template_node_id(&template, template_scope)?;
                metadata.plan = Some(slug.clone());
                metadata.branch = Some(branch.clone());

                let mut primary_node_arg_overrides = BTreeMap::new();
                match (&resolved.origin, input_file.as_ref()) {
                    (InputOrigin::Inline, _) => {
                        primary_node_arg_overrides
                            .insert("spec_text".to_string(), resolved.text.clone());
                        primary_node_arg_overrides
                            .insert("spec_source".to_string(), "inline".to_string());
                    }
                    (InputOrigin::File(path), _) => {
                        primary_node_arg_overrides
                            .insert("spec_file".to_string(), path.display().to_string());
                        primary_node_arg_overrides
                            .insert("spec_source".to_string(), "file".to_string());
                    }
                    (InputOrigin::Stdin, Some(path)) => {
                        primary_node_arg_overrides
                            .insert("spec_file".to_string(), path.display().to_string());
                        primary_node_arg_overrides
                            .insert("spec_source".to_string(), "stdin".to_string());
                    }
                    (InputOrigin::Stdin, None) => {
                        primary_node_arg_overrides
                            .insert("spec_text".to_string(), resolved.text.clone());
                        primary_node_arg_overrides
                            .insert("spec_source".to_string(), "stdin".to_string());
                    }
                }
                primary_node_arg_overrides.insert("name_override".to_string(), slug);
                primary_node_arg_overrides
                    .insert("commit_mode".to_string(), commit_mode.label().to_string());

                enqueue_wrapper_template_graph(EnqueueWrapperTemplateGraphRequest {
                    project_root: &project_root,
                    jobs_root: &jobs_root,
                    root_job_id: &job_id,
                    follow_primary: follow,
                    background: &workflow_defaults.background,
                    config_snapshot: &config_snapshot,
                    requested_after: &requested_after,
                    base_metadata: &metadata,
                    template_ref: &template_ref,
                    scope_label: template_scope.label().to_string(),
                    compat_scope: Some(template_scope),
                    template: &template,
                    primary_node_id: &primary_node_id,
                    pinned_head: None,
                    capture_save_patch_for_primary: false,
                    primary_node_arg_overrides: &primary_node_arg_overrides,
                    passthrough_global_args: &passthrough_global_args,
                    cli_agent_override: cli_agent_override.as_ref(),
                })?;
            }
            Commands::Patch(cmd) => {
                if !cmd.assume_yes && !io::stdin().is_terminal() {
                    return Err("vizier patch requires --yes in scheduler mode".into());
                }
                let (_alias, template_scope, template_ref) = resolve_wrapper_template_ref(
                    &config::get_config(),
                    "patch",
                    TemplateScope::Patch,
                )?;
                let pipeline = match cmd.pipeline {
                    Some(BuildPipelineArg::Approve) => "approve",
                    Some(BuildPipelineArg::ApproveReview) => "approve-review",
                    Some(BuildPipelineArg::ApproveReviewMerge) => "approve-review-merge",
                    None => "approve-review-merge",
                };
                let mut queue_assume_yes = cmd.assume_yes;
                if !cmd.assume_yes {
                    let confirmed = prompt_yes_no(&format!(
                        "Queue patch run for {} file(s) with pipeline {}?",
                        cmd.files.len(),
                        pipeline
                    ))?;
                    if !confirmed {
                        return Err("aborted by user".into());
                    }
                    queue_assume_yes = true;
                }
                let template = resolve_patch_template(
                    &template_ref,
                    pipeline,
                    cmd.target.as_deref(),
                    cmd.resume,
                )?;
                let primary_node_id = resolve_primary_template_node_id(&template, template_scope)?;
                let files_json = serde_json::to_string(
                    &cmd.files
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>(),
                )?;
                let mut primary_node_arg_overrides = BTreeMap::new();
                primary_node_arg_overrides.insert("files_json".to_string(), files_json);
                primary_node_arg_overrides.insert("pipeline".to_string(), pipeline.to_string());
                if let Some(target) = cmd.target.as_ref() {
                    primary_node_arg_overrides.insert("target".to_string(), target.clone());
                }
                primary_node_arg_overrides.insert("resume".to_string(), cmd.resume.to_string());
                primary_node_arg_overrides
                    .insert("assume_yes".to_string(), queue_assume_yes.to_string());
                primary_node_arg_overrides.insert("follow".to_string(), follow.to_string());
                primary_node_arg_overrides
                    .insert("commit_mode".to_string(), commit_mode.label().to_string());

                enqueue_wrapper_template_graph(EnqueueWrapperTemplateGraphRequest {
                    project_root: &project_root,
                    jobs_root: &jobs_root,
                    root_job_id: &job_id,
                    follow_primary: follow,
                    background: &workflow_defaults.background,
                    config_snapshot: &config_snapshot,
                    requested_after: &requested_after,
                    base_metadata: &metadata,
                    template_ref: &template_ref,
                    scope_label: template_scope.label().to_string(),
                    compat_scope: Some(template_scope),
                    template: &template,
                    primary_node_id: &primary_node_id,
                    pinned_head: None,
                    capture_save_patch_for_primary: false,
                    primary_node_arg_overrides: &primary_node_arg_overrides,
                    passthrough_global_args: &passthrough_global_args,
                    cli_agent_override: cli_agent_override.as_ref(),
                })?;
            }
            Commands::Approve(cmd) => {
                if !cmd.assume_yes && !io::stdin().is_terminal() {
                    return Err("vizier approve requires --yes in scheduler mode".into());
                }
                let spec = plan::PlanBranchSpec::resolve(
                    Some(cmd.plan.as_str()),
                    cmd.branch.as_deref(),
                    cmd.target.as_deref(),
                )?;
                let draft_active = has_active_plan_job(&jobs_root, &spec.slug, "draft")?;
                let branch_exists = vcs::branch_exists(&spec.branch)?;
                let plan_doc_exists = plan::load_plan_from_branch(&spec.slug, &spec.branch).is_ok();
                if !plan_doc_exists && !draft_active {
                    return Err(format!(
                        "plan {} not found; run `vizier draft {}` first",
                        spec.slug, spec.slug
                    )
                    .into());
                }
                if !branch_exists && !draft_active {
                    return Err(format!(
                        "draft branch {} not found; run `vizier draft {}` first",
                        spec.branch, spec.slug
                    )
                    .into());
                }
                if !cmd.assume_yes {
                    let confirmed =
                        prompt_yes_no(&format!("Approve plan {} on {}?", spec.slug, spec.branch))?;
                    if !confirmed {
                        return Err("aborted by user".into());
                    }
                }
                let require_human_approval = cmd.require_approval && !cmd.no_require_approval;
                let resolved_approve = resolve_approve_options(cmd, push_after)?;
                let stop_condition_script = resolved_approve
                    .stop_condition
                    .script
                    .as_ref()
                    .map(|path| path.display().to_string());
                let (_alias, template_scope, template_ref) = resolve_wrapper_template_ref(
                    &config::get_config(),
                    "approve",
                    TemplateScope::Approve,
                )?;
                let template = resolve_approve_template(
                    &template_ref,
                    &spec.slug,
                    &spec.branch,
                    &job_id,
                    require_human_approval,
                    stop_condition_script.as_deref(),
                    resolved_approve.stop_condition.retries,
                )?;
                let primary_node_id = resolve_primary_template_node_id(&template, template_scope)?;
                metadata.plan = Some(spec.slug.clone());
                metadata.branch = Some(spec.branch.clone());
                metadata.target = Some(spec.target_branch.clone());
                let mut primary_node_arg_overrides = BTreeMap::new();
                primary_node_arg_overrides.insert("assume_yes".to_string(), "true".to_string());
                primary_node_arg_overrides.insert(
                    "stop_condition_retries".to_string(),
                    resolved_approve.stop_condition.retries.to_string(),
                );
                if let Some(script) = stop_condition_script {
                    primary_node_arg_overrides.insert("stop_condition_script".to_string(), script);
                }
                primary_node_arg_overrides.insert(
                    "push_after".to_string(),
                    resolved_approve.push_after.to_string(),
                );
                primary_node_arg_overrides
                    .insert("commit_mode".to_string(), commit_mode.label().to_string());

                enqueue_wrapper_template_graph(EnqueueWrapperTemplateGraphRequest {
                    project_root: &project_root,
                    jobs_root: &jobs_root,
                    root_job_id: &job_id,
                    follow_primary: follow,
                    background: &workflow_defaults.background,
                    config_snapshot: &config_snapshot,
                    requested_after: &requested_after,
                    base_metadata: &metadata,
                    template_ref: &template_ref,
                    scope_label: template_scope.label().to_string(),
                    compat_scope: Some(template_scope),
                    template: &template,
                    primary_node_id: &primary_node_id,
                    pinned_head: None,
                    capture_save_patch_for_primary: false,
                    primary_node_arg_overrides: &primary_node_arg_overrides,
                    passthrough_global_args: &passthrough_global_args,
                    cli_agent_override: cli_agent_override.as_ref(),
                })?;
            }
            Commands::Review(cmd) => {
                if !cmd.assume_yes
                    && !cmd.review_only
                    && !cmd.review_file
                    && !io::stdin().is_terminal()
                {
                    return Err(
                        "vizier review requires --yes, --review-only, or --review-file in scheduler mode".into(),
                    );
                }
                let plan_slug = cmd
                    .plan
                    .as_deref()
                    .ok_or("plan argument is required for vizier review")?;
                let spec = plan::PlanBranchSpec::resolve(
                    Some(plan_slug),
                    cmd.branch.as_deref(),
                    cmd.target.as_deref(),
                )?;
                let draft_active = has_active_plan_job(&jobs_root, &spec.slug, "draft")?;
                let branch_exists = vcs::branch_exists(&spec.branch)?;
                let plan_doc_exists = plan::load_plan_from_branch(&spec.slug, &spec.branch).is_ok();
                if !plan_doc_exists && !draft_active {
                    return Err(format!(
                        "plan {} not found; run `vizier draft {}` first",
                        spec.slug, spec.slug
                    )
                    .into());
                }
                if !branch_exists && !draft_active {
                    return Err(format!(
                        "draft branch {} not found; run `vizier draft {}` first",
                        spec.branch, spec.slug
                    )
                    .into());
                }
                let mut queue_assume_yes = cmd.assume_yes;
                let mut queue_review_only = cmd.review_only;
                let mut queue_review_file = cmd.review_file;
                if !cmd.assume_yes && !cmd.review_only && !cmd.review_file {
                    match prompt_review_queue_choice()? {
                        ReviewQueueChoice::ApplyFixes => {
                            queue_assume_yes = true;
                        }
                        ReviewQueueChoice::CritiqueOnly => {
                            queue_review_only = true;
                        }
                        ReviewQueueChoice::ReviewFile => {
                            queue_review_file = true;
                        }
                        ReviewQueueChoice::Cancel => {
                            return Err("aborted by user".into());
                        }
                    }
                }
                let resolved_review = resolve_review_options(
                    &ReviewCmd {
                        plan: cmd.plan.clone(),
                        target: cmd.target.clone(),
                        branch: cmd.branch.clone(),
                        assume_yes: queue_assume_yes,
                        review_only: queue_review_only,
                        review_file: queue_review_file,
                        skip_checks: cmd.skip_checks,
                        cicd_script: cmd.cicd_script.clone(),
                        auto_cicd_fix: cmd.auto_cicd_fix,
                        no_auto_cicd_fix: cmd.no_auto_cicd_fix,
                        cicd_retries: cmd.cicd_retries,
                        after: cmd.after.clone(),
                    },
                    push_after,
                )?;
                let review_gate_script = resolved_review
                    .cicd_gate
                    .script
                    .as_ref()
                    .map(|path| path.display().to_string());
                let (_alias, template_scope, template_ref) = resolve_wrapper_template_ref(
                    &config::get_config(),
                    "review",
                    TemplateScope::Review,
                )?;
                let template = resolve_review_template(
                    &template_ref,
                    &spec.slug,
                    &spec.branch,
                    &job_id,
                    review_gate_script.as_deref(),
                )?;
                let primary_node_id = resolve_primary_template_node_id(&template, template_scope)?;
                metadata.plan = Some(spec.slug.clone());
                metadata.branch = Some(spec.branch.clone());
                metadata.target = Some(spec.target_branch.clone());
                let mut primary_node_arg_overrides = BTreeMap::new();
                primary_node_arg_overrides
                    .insert("assume_yes".to_string(), queue_assume_yes.to_string());
                primary_node_arg_overrides
                    .insert("review_only".to_string(), queue_review_only.to_string());
                primary_node_arg_overrides
                    .insert("review_file".to_string(), queue_review_file.to_string());
                primary_node_arg_overrides.insert(
                    "skip_checks".to_string(),
                    resolved_review.skip_checks.to_string(),
                );
                primary_node_arg_overrides.insert(
                    "auto_resolve_requested".to_string(),
                    resolved_review.auto_resolve_requested.to_string(),
                );
                if let Some(script) = review_gate_script {
                    primary_node_arg_overrides.insert("cicd_script".to_string(), script);
                }
                primary_node_arg_overrides.insert(
                    "cicd_auto_resolve".to_string(),
                    resolved_review.cicd_gate.auto_resolve.to_string(),
                );
                primary_node_arg_overrides.insert(
                    "cicd_retries".to_string(),
                    resolved_review.cicd_gate.retries.to_string(),
                );
                primary_node_arg_overrides.insert(
                    "push_after".to_string(),
                    resolved_review.push_after.to_string(),
                );
                primary_node_arg_overrides
                    .insert("commit_mode".to_string(), commit_mode.label().to_string());

                enqueue_wrapper_template_graph(EnqueueWrapperTemplateGraphRequest {
                    project_root: &project_root,
                    jobs_root: &jobs_root,
                    root_job_id: &job_id,
                    follow_primary: follow,
                    background: &workflow_defaults.background,
                    config_snapshot: &config_snapshot,
                    requested_after: &requested_after,
                    base_metadata: &metadata,
                    template_ref: &template_ref,
                    scope_label: template_scope.label().to_string(),
                    compat_scope: Some(template_scope),
                    template: &template,
                    primary_node_id: &primary_node_id,
                    pinned_head: None,
                    capture_save_patch_for_primary: false,
                    primary_node_arg_overrides: &primary_node_arg_overrides,
                    passthrough_global_args: &passthrough_global_args,
                    cli_agent_override: cli_agent_override.as_ref(),
                })?;
            }
            Commands::Merge(cmd) => {
                if !cmd.assume_yes && !io::stdin().is_terminal() {
                    return Err("vizier merge requires --yes in scheduler mode".into());
                }
                let plan_slug = cmd
                    .plan
                    .as_deref()
                    .ok_or("plan argument is required for vizier merge")?;
                let spec = plan::PlanBranchSpec::resolve(
                    Some(plan_slug),
                    cmd.branch.as_deref(),
                    cmd.target.as_deref(),
                )?;
                let draft_active = has_active_plan_job(&jobs_root, &spec.slug, "draft")?;
                let branch_exists = vcs::branch_exists(&spec.branch)?;
                if !branch_exists && !draft_active {
                    return Err(format!(
                        "draft branch {} not found; run `vizier draft {}` first",
                        spec.branch, spec.slug
                    )
                    .into());
                }
                if !cmd.assume_yes {
                    let confirmed = prompt_yes_no(&format!(
                        "Merge plan {} into {}?",
                        spec.slug, spec.target_branch
                    ))?;
                    if !confirmed {
                        return Err("aborted by user".into());
                    }
                }
                let resolved_merge = resolve_merge_options(cmd, push_after)?;
                let merge_gate_script = resolved_merge
                    .cicd_gate
                    .script
                    .as_ref()
                    .map(|path| path.display().to_string());
                let (_alias, template_scope, template_ref) = resolve_wrapper_template_ref(
                    &config::get_config(),
                    "merge",
                    TemplateScope::Merge,
                )?;
                let template = resolve_merge_template(
                    &template_ref,
                    &spec.slug,
                    &spec.branch,
                    &spec.target_branch,
                    MergeTemplateGateConfig {
                        cicd_script: merge_gate_script.as_deref(),
                        cicd_auto_resolve: resolved_merge.cicd_gate.auto_resolve,
                        cicd_retries: resolved_merge.cicd_gate.retries,
                        conflict_auto_resolve: resolved_merge.conflict_auto_resolve.enabled(),
                    },
                )?;
                let primary_node_id = resolve_primary_template_node_id(&template, template_scope)?;
                metadata.plan = Some(spec.slug.clone());
                metadata.branch = Some(spec.branch.clone());
                metadata.target = Some(spec.target_branch.clone());
                let mut primary_node_arg_overrides = BTreeMap::new();
                primary_node_arg_overrides.insert("assume_yes".to_string(), "true".to_string());
                primary_node_arg_overrides.insert(
                    "delete_branch".to_string(),
                    resolved_merge.delete_branch.to_string(),
                );
                if let Some(note) = resolved_merge.note.as_ref() {
                    primary_node_arg_overrides.insert("note".to_string(), note.clone());
                }
                primary_node_arg_overrides.insert(
                    "push_after".to_string(),
                    resolved_merge.push_after.to_string(),
                );
                primary_node_arg_overrides.insert(
                    "conflict_auto_resolve".to_string(),
                    resolved_merge.conflict_auto_resolve.enabled().to_string(),
                );
                primary_node_arg_overrides.insert(
                    "conflict_auto_resolve_source".to_string(),
                    resolved_merge
                        .conflict_auto_resolve
                        .source_description()
                        .to_string(),
                );
                primary_node_arg_overrides.insert(
                    "complete_conflict".to_string(),
                    resolved_merge.complete_conflict.to_string(),
                );
                if let Some(script) = merge_gate_script {
                    primary_node_arg_overrides.insert("cicd_script".to_string(), script);
                }
                primary_node_arg_overrides.insert(
                    "cicd_auto_resolve".to_string(),
                    resolved_merge.cicd_gate.auto_resolve.to_string(),
                );
                primary_node_arg_overrides.insert(
                    "cicd_retries".to_string(),
                    resolved_merge.cicd_gate.retries.to_string(),
                );
                primary_node_arg_overrides
                    .insert("squash".to_string(), resolved_merge.squash.to_string());
                if let Some(mainline) = resolved_merge.squash_mainline {
                    primary_node_arg_overrides
                        .insert("squash_mainline".to_string(), mainline.to_string());
                }
                primary_node_arg_overrides
                    .insert("commit_mode".to_string(), commit_mode.label().to_string());

                enqueue_wrapper_template_graph(EnqueueWrapperTemplateGraphRequest {
                    project_root: &project_root,
                    jobs_root: &jobs_root,
                    root_job_id: &job_id,
                    follow_primary: follow,
                    background: &workflow_defaults.background,
                    config_snapshot: &config_snapshot,
                    requested_after: &requested_after,
                    base_metadata: &metadata,
                    template_ref: &template_ref,
                    scope_label: template_scope.label().to_string(),
                    compat_scope: Some(template_scope),
                    template: &template,
                    primary_node_id: &primary_node_id,
                    pinned_head: None,
                    capture_save_patch_for_primary: false,
                    primary_node_arg_overrides: &primary_node_arg_overrides,
                    passthrough_global_args: &passthrough_global_args,
                    cli_agent_override: cli_agent_override.as_ref(),
                })?;
            }
            Commands::Run(cmd) => {
                let alias = parse_command_alias(&cmd.alias)?;
                let mut runtime_params = parse_runtime_param_overrides(&cmd.set)?;

                let include_prompt_input = cmd.spec.is_some() || cmd.file.is_some();
                if include_prompt_input {
                    let (resolved, input_file) = prepare_prompt_input(
                        cmd.spec.as_deref(),
                        cmd.file.as_deref(),
                        &project_root,
                        &job_id,
                    )?;
                    if !runtime_params.contains_key("spec_source") {
                        match (&resolved.origin, input_file.as_ref()) {
                            (InputOrigin::Inline, _) => {
                                runtime_params
                                    .insert("spec_source".to_string(), "inline".to_string());
                                runtime_params
                                    .entry("spec_text".to_string())
                                    .or_insert(resolved.text.clone());
                            }
                            (InputOrigin::File(path), _) => {
                                runtime_params
                                    .insert("spec_source".to_string(), "file".to_string());
                                runtime_params
                                    .entry("spec_file".to_string())
                                    .or_insert(path.display().to_string());
                            }
                            (InputOrigin::Stdin, Some(path)) => {
                                runtime_params
                                    .insert("spec_source".to_string(), "stdin".to_string());
                                runtime_params
                                    .entry("spec_file".to_string())
                                    .or_insert(path.display().to_string());
                            }
                            (InputOrigin::Stdin, None) => {
                                runtime_params
                                    .insert("spec_source".to_string(), "stdin".to_string());
                                runtime_params
                                    .entry("spec_text".to_string())
                                    .or_insert(resolved.text.clone());
                            }
                        }
                    }
                }

                let resolved_slug = resolve_run_slug(&runtime_params, cmd.name.as_deref())?;
                let plan_dir = project_root.join(".vizier/implementation-plans");
                std::fs::create_dir_all(&plan_dir)?;
                let slug = plan::ensure_unique_slug(&resolved_slug, &plan_dir, "draft/")?;
                runtime_params
                    .entry("slug".to_string())
                    .or_insert(slug.clone());
                runtime_params
                    .entry("plan".to_string())
                    .or_insert(slug.clone());

                let branch = runtime_params
                    .get("branch")
                    .cloned()
                    .or_else(|| cmd.branch.clone())
                    .unwrap_or_else(|| plan::default_branch_for_slug(&slug));
                runtime_params
                    .entry("branch".to_string())
                    .or_insert(branch.clone());

                let target = runtime_params
                    .get("target")
                    .cloned()
                    .or_else(|| cmd.target.clone())
                    .or_else(vcs::detect_primary_branch)
                    .unwrap_or_else(|| "main".to_string());
                runtime_params
                    .entry("target".to_string())
                    .or_insert(target.clone());

                let cfg = config::get_config();
                runtime_params
                    .entry("commit_mode".to_string())
                    .or_insert(commit_mode.label().to_string());
                runtime_params
                    .entry("push_after".to_string())
                    .or_insert(push_after.to_string());
                runtime_params
                    .entry("assume_yes".to_string())
                    .or_insert("true".to_string());
                runtime_params
                    .entry("delete_branch".to_string())
                    .or_insert("true".to_string());
                runtime_params
                    .entry("complete_conflict".to_string())
                    .or_insert("false".to_string());
                runtime_params
                    .entry("stop_condition_retries".to_string())
                    .or_insert(cfg.approve.stop_condition.retries.to_string());
                if let Some(script) = cfg.approve.stop_condition.script.as_ref() {
                    runtime_params
                        .entry("stop_condition_script".to_string())
                        .or_insert(script.display().to_string());
                }
                runtime_params
                    .entry("cicd_auto_resolve".to_string())
                    .or_insert(cfg.merge.cicd_gate.auto_resolve.to_string());
                runtime_params
                    .entry("cicd_retries".to_string())
                    .or_insert(cfg.merge.cicd_gate.retries.to_string());
                if let Some(script) = cfg.merge.cicd_gate.script.as_ref() {
                    runtime_params
                        .entry("cicd_script".to_string())
                        .or_insert(script.display().to_string());
                }
                runtime_params
                    .entry("conflict_auto_resolve".to_string())
                    .or_insert(cfg.merge.conflicts.auto_resolve.to_string());
                runtime_params
                    .entry("conflict_auto_resolve_source".to_string())
                    .or_insert("merge.conflicts.auto_resolve".to_string());
                runtime_params
                    .entry("squash".to_string())
                    .or_insert(cfg.merge.squash_default.to_string());
                if let Some(mainline) = cfg.merge.squash_mainline {
                    runtime_params
                        .entry("squash_mainline".to_string())
                        .or_insert(mainline.to_string());
                }

                let (template_ref, mut template) =
                    resolve_custom_alias_template(&cfg, &alias, &runtime_params)?;
                strip_empty_runtime_args(&mut template);
                validate_template_agent_backends(&template, &cfg, cli_agent_override.as_ref())?;

                let node_schedule = compile_template_node_schedule(&template)?;
                let primary_node_id = select_primary_run_node_id(&template, &node_schedule)?;
                metadata.command_alias = Some(alias.to_string());
                metadata.scope = Some(alias.to_string());
                metadata.plan = Some(slug.clone());
                metadata.branch = Some(branch.clone());
                metadata.target = Some(target.clone());

                let primary_node_arg_overrides = BTreeMap::new();
                enqueue_wrapper_template_graph(EnqueueWrapperTemplateGraphRequest {
                    project_root: &project_root,
                    jobs_root: &jobs_root,
                    root_job_id: &job_id,
                    follow_primary: follow,
                    background: &workflow_defaults.background,
                    config_snapshot: &config_snapshot,
                    requested_after: &requested_after,
                    base_metadata: &metadata,
                    template_ref: &template_ref,
                    scope_label: alias.to_string(),
                    compat_scope: None,
                    template: &template,
                    primary_node_id: &primary_node_id,
                    pinned_head: None,
                    capture_save_patch_for_primary: false,
                    primary_node_arg_overrides: &primary_node_arg_overrides,
                    passthrough_global_args: &passthrough_global_args,
                    cli_agent_override: cli_agent_override.as_ref(),
                })?;
            }
            _ => {}
        }

        let binary = std::env::current_exe()?;
        let _ = jobs::scheduler_tick(&project_root, &jobs_root, &binary);

        let record = jobs::read_record(&jobs_root, &job_id)?;
        emit_job_summary(&record);

        if follow {
            let exit_code = jobs::follow_job_logs_raw(&jobs_root, &job_id)?;
            std::process::exit(exit_code);
        }

        return Ok(());
    }

    let result = (async {
        match cli.command {
            Commands::Help(_) => Ok(()),
            Commands::Init(cmd) => run_init(&project_root, cmd.check),
            Commands::Completions(cmd) => {
                crate::completions::write_registration(cmd.shell.into(), Cli::command)?;
                Ok(())
            }
            Commands::Complete(_) => Ok(()),
            Commands::WorkflowNode(cmd) => {
                run_workflow_node(
                    WorkflowNodeArgs {
                        scope: cmd.scope,
                        build_id: cmd.build_id,
                        step_key: cmd.step_key,
                        node_id: cmd.node_id,
                        slug: cmd.slug,
                        branch: cmd.branch,
                        target: cmd.target,
                        node_json: cmd.node_json,
                    },
                    &project_root,
                )
                .await
            }

            Commands::Save(SaveCmd {
                rev_or_range,
                commit_message,
                commit_message_editor,
                after: _,
            }) => {
                let agent = resolve_wrapper_agent(
                    &config::get_config(),
                    "save",
                    cli_agent_override.as_ref(),
                )?;
                if let Some(job_id) = cli.global.background_job_id.as_deref() {
                    let cmd = SaveCmd {
                        rev_or_range,
                        commit_message,
                        commit_message_editor,
                        after: Vec::new(),
                    };
                    run_scheduled_save(
                        job_id,
                        &cmd,
                        push_after,
                        commit_mode,
                        &agent,
                        &project_root,
                        &jobs_root,
                    )
                    .await
                } else {
                    run_save(
                        &rev_or_range,
                        &[".vizier/"],
                        commit_message,
                        commit_message_editor,
                        commit_mode,
                        push_after,
                        &agent,
                    )
                    .await
                }
            }

            Commands::TestDisplay(cmd) => {
                let opts = resolve_test_display_options(&cmd)?;
                let cfg = config::get_config();
                let agent = config::resolve_agent_settings_for_alias(
                    &cfg,
                    &opts.command_alias,
                    cli_agent_override.as_ref(),
                )?;
                run_test_display(opts, &agent).await
            }

            Commands::Draft(cmd) => {
                let resolved = resolve_draft_spec(&cmd)?;
                let agent = resolve_wrapper_agent(
                    &config::get_config(),
                    "draft",
                    cli_agent_override.as_ref(),
                )?;
                run_draft(
                    DraftArgs {
                        spec_text: resolved.text,
                        spec_source: resolved.origin.into(),
                        name_override: cmd.name.clone(),
                    },
                    &agent,
                    commit_mode,
                )
                .await
            }

            Commands::Build(BuildCmd {
                file,
                name,
                command,
            }) => {
                if command.is_some() && (file.is_some() || name.is_some()) {
                    return Err(
                        "`vizier build --file/--name` cannot be combined with subcommands (`execute`, `__materialize`, `__template-node`)."
                            .into(),
                    );
                }
                match command {
                    Some(BuildActionCmd::Execute(exec)) => {
                    let pipeline = exec.pipeline.map(|value| match value {
                        BuildPipelineArg::Approve => BuildExecutionPipeline::Approve,
                        BuildPipelineArg::ApproveReview => BuildExecutionPipeline::ApproveReview,
                        BuildPipelineArg::ApproveReviewMerge => {
                            BuildExecutionPipeline::ApproveReviewMerge
                        }
                    });
                    run_build_execute(
                        BuildExecuteArgs {
                            build_id: exec.build_id,
                            pipeline_override: pipeline,
                            target_override: None,
                            resume: exec.resume,
                            assume_yes: exec.assume_yes,
                            follow: cli.global.follow,
                            requested_after: &[],
                        },
                        &project_root,
                    )
                    .await
                }
                    Some(BuildActionCmd::Materialize(materialize)) => {
                    run_build_materialize(
                        materialize.build_id,
                        materialize.step_key,
                        materialize.slug,
                        materialize.branch,
                        materialize.target,
                        &project_root,
                    )
                    .await
                }
                    Some(BuildActionCmd::TemplateNode(node)) => {
                    run_build_template_node(
                        BuildTemplateNodeArgs {
                            build_id: node.build_id,
                            step_key: node.step_key,
                            node_id: node.node_id,
                            slug: node.slug,
                            branch: node.branch,
                            target: node.target,
                            node_json: node.node_json,
                        },
                        &project_root,
                    )
                    .await
                }
                    None => {
                    let build_file =
                        file.ok_or("vizier build requires --file when no subcommand is used")?;
                    let agent = resolve_wrapper_agent(
                        &config::get_config(),
                        "build_execute",
                        cli_agent_override.as_ref(),
                    )?;
                    run_build(build_file, name, &project_root, &agent, commit_mode).await
                }
                }
            }

            Commands::Patch(cmd) => {
                let pipeline = cmd.pipeline.map(|value| match value {
                    BuildPipelineArg::Approve => BuildExecutionPipeline::Approve,
                    BuildPipelineArg::ApproveReview => BuildExecutionPipeline::ApproveReview,
                    BuildPipelineArg::ApproveReviewMerge => {
                        BuildExecutionPipeline::ApproveReviewMerge
                    }
                });
                let agent = resolve_wrapper_agent(
                    &config::get_config(),
                    "patch",
                    cli_agent_override.as_ref(),
                )?;
                run_patch(
                    PatchArgs {
                        files: cmd.files,
                        pipeline,
                        target: cmd.target,
                        resume: cmd.resume,
                        assume_yes: cmd.assume_yes,
                        follow: cli.global.follow,
                        after: cmd.after,
                    },
                    &project_root,
                    &agent,
                    commit_mode,
                )
                .await
            }

            Commands::List(cmd) => run_list(resolve_list_options(&cmd)?),
            Commands::Cd(cmd) => run_cd(resolve_cd_options(&cmd)?),
            Commands::Clean(cmd) => run_clean(resolve_clean_options(&cmd)?),
            Commands::Plan(cmd) => run_plan_summary(cli_agent_override.as_ref(), cmd.json),
            Commands::Jobs(cmd) => run_jobs_command(
                &project_root,
                &jobs_root,
                cmd,
                cli.global.follow,
                cli.global.no_ansi,
            ),

            Commands::Approve(cmd) => {
                let opts = resolve_approve_options(&cmd, push_after)?;
                let agent = resolve_wrapper_agent(
                    &config::get_config(),
                    "approve",
                    cli_agent_override.as_ref(),
                )?;
                run_approve(opts, &agent, commit_mode).await
            }
            Commands::Review(cmd) => {
                let opts = resolve_review_options(&cmd, push_after)?;
                let agent = resolve_wrapper_agent(
                    &config::get_config(),
                    "review",
                    cli_agent_override.as_ref(),
                )?;
                run_review(opts, &agent, commit_mode).await
            }
            Commands::Merge(cmd) => {
                let opts = resolve_merge_options(&cmd, push_after)?;
                let agent = resolve_wrapper_agent(
                    &config::get_config(),
                    "merge",
                    cli_agent_override.as_ref(),
                )?;
                run_merge(opts, &agent, commit_mode).await
            }
            Commands::Run(_) => {
                Err("`vizier run` executes through the scheduler; invoke it without --background-job-id".into())
            }
            Commands::Release(cmd) => run_release(cmd),
        }
    })
    .await;

    let cancelled = result
        .as_ref()
        .err()
        .and_then(|err| err.downcast_ref::<CancelledError>())
        .is_some();

    if let Some(job_id) = cli.global.background_job_id.as_ref() {
        let success = result.is_ok();
        let status = if success {
            jobs::JobStatus::Succeeded
        } else {
            jobs::JobStatus::Failed
        };

        let mut exit_code = if success { 0 } else { 1 };
        if let Some(run) = auditor::Auditor::latest_agent_run()
            && (success || run.exit_code != 0)
        {
            exit_code = run.exit_code;
        }

        let session_path = auditor::Auditor::persist_session_log().map(|artifact| {
            auditor_cleanup.persisted = true;
            display::info(format!("Session saved to {}", artifact.display_path()));
            artifact.display_path()
        });

        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();

        let finalized = match jobs::finalize_job(
            &project_root,
            &jobs_root,
            job_id,
            status,
            exit_code,
            session_path,
            runtime_job_metadata(),
        ) {
            Ok(record) => Some(record),
            Err(err) => {
                display::warn(format!(
                    "unable to update background job {} status: {}",
                    job_id, err
                ));
                None
            }
        };

        if finalized.is_some()
            && let Ok(binary) = std::env::current_exe()
            && let Err(err) = jobs::scheduler_tick(&project_root, &jobs_root, &binary)
        {
            display::warn(format!("unable to advance scheduler: {err}"));
        }
    }

    if cancelled {
        let _ = std::io::stdout().flush();
        std::process::exit(1);
    }

    result
}

fn legacy_global_arg_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-C" | "--config-file"
            | "-l"
            | "--load-session"
            | "--agent"
            | "--agent-label"
            | "--agent-command"
            | "--background-job-id"
    )
}

fn first_subcommand_index(raw_args: &[String]) -> Option<usize> {
    let mut index = 1usize;
    while index < raw_args.len() {
        let arg = &raw_args[index];
        if arg == "--" {
            break;
        }

        if let Some((flag, _)) = arg.split_once('=')
            && legacy_global_arg_takes_value(flag)
        {
            index += 1;
            continue;
        }

        if arg.starts_with('-') {
            index += 1;
            if legacy_global_arg_takes_value(arg) {
                index += 1;
            }
            continue;
        }

        return Some(index);
    }

    None
}

fn removed_global_flag_guidance(raw_args: &[String]) -> Option<String> {
    let subcommand = first_subcommand_index(raw_args)
        .and_then(|index| raw_args.get(index).map(|value| (index, value.as_str())));

    for (index, arg) in raw_args.iter().enumerate().skip(1) {
        let flag = arg.split_once('=').map(|(name, _)| name).unwrap_or(arg);
        match flag {
            "--background" => {
                return Some(
                    "`--background` was removed; assistant-backed commands already run as scheduled jobs. Use `--follow` to stream logs.".to_string(),
                );
            }
            "--no-background" => {
                return Some(
                    "`--no-background` was removed; foreground execution is no longer supported for assistant-backed commands.".to_string(),
                );
            }
            "--pager" => {
                return Some(
                    "`--pager` was removed; help output pages automatically on TTY and respects `$VIZIER_PAGER`. Use `--no-pager` to disable paging."
                        .to_string(),
                );
            }
            "--agent-label" => {
                return Some(
                    "`--agent-label` was removed; set the shim label in config (`[agents.commands.<alias>.agent] label = \"...\"`) and use `--config-file` for ad-hoc runs."
                        .to_string(),
                );
            }
            "--agent-command" => {
                return Some(
                    "`--agent-command` was removed; set runtime commands in config (`[agents.commands.<alias>.agent] command = [\"...\"]`) and use `--config-file` for ad-hoc runs."
                        .to_string(),
                );
            }
            "-j" | "--json" => {
                let plan_local = subcommand
                    .map(|(sub_index, name)| name == "plan" && index > sub_index)
                    .unwrap_or(false);
                if !plan_local {
                    return Some(
                        "global `--json` was removed; use command-local output selectors (`vizier list --format json`, `vizier jobs ... --format json`) or `vizier plan --json`."
                            .to_string(),
                    );
                }
            }
            _ => {}
        }
    }

    None
}

fn workflow_node_passthrough_global_args(global: &GlobalOpts) -> Vec<String> {
    let mut args = Vec::new();
    if global.quiet {
        args.push("--quiet".to_string());
    }
    for _ in 0..global.verbose {
        args.push("-v".to_string());
    }
    if global.debug {
        args.push("--debug".to_string());
    }
    if global.no_ansi {
        args.push("--no-ansi".to_string());
    }
    if let Some(session_id) = global.load_session.as_ref() {
        args.push("--load-session".to_string());
        args.push(session_id.clone());
    }
    if global.no_session {
        args.push("--no-session".to_string());
    }
    if let Some(agent) = global.agent.as_ref() {
        args.push("--agent".to_string());
        args.push(agent.clone());
    }
    if let Some(config_file) = global.config_file.as_ref() {
        args.push("--config-file".to_string());
        args.push(config_file.clone());
    }
    if global.push {
        args.push("--push".to_string());
    }
    if global.no_commit {
        args.push("--no-commit".to_string());
    }
    args
}

fn parse_command_alias(alias: &str) -> Result<config::CommandAlias, Box<dyn std::error::Error>> {
    alias.parse::<config::CommandAlias>().map_err(|err| {
        Box::<dyn std::error::Error>::from(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid command alias `{alias}`: {err}"),
        ))
    })
}

fn parse_runtime_param_overrides(
    entries: &[String],
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut parsed = BTreeMap::new();
    for entry in entries {
        let (key, value) = entry.split_once('=').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid --set value `{entry}`; expected KEY=VALUE"),
            )
        })?;
        let key = key.trim();
        if key.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid --set value `{entry}`; key cannot be empty"),
            )
            .into());
        }
        parsed.insert(key.to_string(), value.to_string());
    }
    Ok(parsed)
}

fn resolve_run_slug(
    runtime_params: &BTreeMap<String, String>,
    explicit_name: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(slug) = runtime_params
        .get("slug")
        .or_else(|| runtime_params.get("plan"))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return plan::sanitize_name_override(slug).map_err(|err| {
            Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
        });
    }
    if let Some(name) = explicit_name {
        return plan::sanitize_name_override(name).map_err(|err| {
            Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
        });
    }
    if let Some(name_override) = runtime_params
        .get("name_override")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return plan::sanitize_name_override(name_override).map_err(|err| {
            Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
        });
    }
    if let Some(spec_text) = runtime_params
        .get("spec_text")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return Ok(plan::slug_from_spec(spec_text));
    }
    Ok("draft-plan".to_string())
}

fn strip_empty_runtime_args(template: &mut WorkflowTemplate) {
    for node in &mut template.nodes {
        node.args.retain(|_, value| !value.trim().is_empty());
        for precondition in &mut node.preconditions {
            if let WorkflowPrecondition::Custom { args, .. } = precondition {
                args.retain(|_, value| !value.trim().is_empty());
            }
        }
        for gate in &mut node.gates {
            if let WorkflowGate::Custom { args, .. } = gate {
                args.retain(|_, value| !value.trim().is_empty());
            }
        }
    }
}

fn select_primary_run_node_id(
    template: &WorkflowTemplate,
    schedule: &WorkflowTemplateNodeSchedule,
) -> Result<String, Box<dyn std::error::Error>> {
    let roots = template
        .nodes
        .iter()
        .filter_map(|node| {
            if node.after.is_empty() {
                Some(node.id.clone())
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();
    if roots.is_empty() {
        return Err(format!(
            "workflow template {}@{} has no root node",
            template.id, template.version
        )
        .into());
    }
    for node_id in &schedule.order {
        if roots.contains(node_id) {
            return Ok(node_id.clone());
        }
    }
    Err(format!(
        "workflow template {}@{} root nodes are missing from the compiled schedule",
        template.id, template.version
    )
    .into())
}

fn resolve_wrapper_template_ref(
    cfg: &config::Config,
    alias_name: &str,
    fallback_scope: TemplateScope,
) -> Result<(config::CommandAlias, TemplateScope, WorkflowTemplateRef), Box<dyn std::error::Error>>
{
    let alias = parse_command_alias(alias_name)?;
    if let Some((scope, template_ref)) = resolve_template_ref_for_alias(cfg, &alias) {
        Ok((alias, scope, template_ref))
    } else {
        Ok((
            alias,
            fallback_scope,
            resolve_template_ref(cfg, fallback_scope),
        ))
    }
}

fn resolve_wrapper_agent(
    cfg: &config::Config,
    alias_name: &str,
    cli_agent_override: Option<&config::AgentOverrides>,
) -> Result<config::AgentSettings, Box<dyn std::error::Error>> {
    let alias = parse_command_alias(alias_name)?;
    config::resolve_agent_settings_for_alias(cfg, &alias, cli_agent_override)
}

fn apply_background_config_snapshot(cfg: &mut config::Config, snapshot: &serde_json::Value) {
    if let Some(selector) = snapshot
        .get("agent_selector")
        .and_then(serde_json::Value::as_str)
        && !selector.trim().is_empty()
    {
        cfg.agent_selector = selector.to_string();
        cfg.backend = config::backend_kind_for_selector(&cfg.agent_selector);
    }
    if let Some(label) = snapshot
        .pointer("/agent/label")
        .and_then(serde_json::Value::as_str)
    {
        let trimmed = label.trim();
        cfg.agent_runtime.label = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
    if let Some(command) = snapshot
        .pointer("/agent/command")
        .and_then(serde_json::Value::as_array)
    {
        let resolved = command
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        cfg.agent_runtime.command = resolved;
    }

    if let Some(commands) = snapshot
        .get("commands")
        .and_then(serde_json::Value::as_object)
    {
        cfg.commands.clear();
        for (raw_alias, raw_selector) in commands {
            let Some(selector) = raw_selector.as_str() else {
                continue;
            };
            if let (Ok(alias), Ok(template_selector)) = (
                raw_alias.parse::<config::CommandAlias>(),
                selector.parse::<config::TemplateSelector>(),
            ) {
                cfg.commands.insert(alias, template_selector);
            }
        }
    }

    if let Some(no_commit_default) = snapshot
        .pointer("/workflow/no_commit_default")
        .and_then(serde_json::Value::as_bool)
    {
        cfg.workflow.no_commit_default = no_commit_default;
    }
    if let Some(enabled) = snapshot
        .pointer("/workflow/background/enabled")
        .and_then(serde_json::Value::as_bool)
    {
        cfg.workflow.background.enabled = enabled;
    }
    if let Some(quiet) = snapshot
        .pointer("/workflow/background/quiet")
        .and_then(serde_json::Value::as_bool)
    {
        cfg.workflow.background.quiet = quiet;
    }

    let templates = &mut cfg.workflow.templates;
    if let Some(value) = snapshot
        .pointer("/workflow/templates/save")
        .and_then(serde_json::Value::as_str)
        && !value.trim().is_empty()
    {
        templates.save = value.to_string();
    }
    if let Some(value) = snapshot
        .pointer("/workflow/templates/draft")
        .and_then(serde_json::Value::as_str)
        && !value.trim().is_empty()
    {
        templates.draft = value.to_string();
    }
    if let Some(value) = snapshot
        .pointer("/workflow/templates/approve")
        .and_then(serde_json::Value::as_str)
        && !value.trim().is_empty()
    {
        templates.approve = value.to_string();
    }
    if let Some(value) = snapshot
        .pointer("/workflow/templates/review")
        .and_then(serde_json::Value::as_str)
        && !value.trim().is_empty()
    {
        templates.review = value.to_string();
    }
    if let Some(value) = snapshot
        .pointer("/workflow/templates/merge")
        .and_then(serde_json::Value::as_str)
        && !value.trim().is_empty()
    {
        templates.merge = value.to_string();
    }
    if let Some(value) = snapshot
        .pointer("/workflow/templates/build_execute")
        .and_then(serde_json::Value::as_str)
        && !value.trim().is_empty()
    {
        templates.build_execute = value.to_string();
    }
    if let Some(value) = snapshot
        .pointer("/workflow/templates/patch")
        .and_then(serde_json::Value::as_str)
        && !value.trim().is_empty()
    {
        templates.patch = value.to_string();
    }
}

struct EnqueueWrapperTemplateGraphRequest<'a> {
    project_root: &'a std::path::Path,
    jobs_root: &'a std::path::Path,
    root_job_id: &'a str,
    follow_primary: bool,
    background: &'a config::BackgroundConfig,
    config_snapshot: &'a serde_json::Value,
    requested_after: &'a [String],
    base_metadata: &'a jobs::JobMetadata,
    template_ref: &'a WorkflowTemplateRef,
    scope_label: String,
    compat_scope: Option<TemplateScope>,
    template: &'a WorkflowTemplate,
    primary_node_id: &'a str,
    pinned_head: Option<jobs::PinnedHead>,
    capture_save_patch_for_primary: bool,
    primary_node_arg_overrides: &'a BTreeMap<String, String>,
    passthrough_global_args: &'a [String],
    cli_agent_override: Option<&'a config::AgentOverrides>,
}

fn enqueue_wrapper_template_graph(
    request: EnqueueWrapperTemplateGraphRequest<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let EnqueueWrapperTemplateGraphRequest {
        project_root,
        jobs_root,
        root_job_id,
        follow_primary,
        background,
        config_snapshot,
        requested_after,
        base_metadata,
        template_ref,
        scope_label,
        compat_scope,
        template,
        primary_node_id,
        pinned_head,
        capture_save_patch_for_primary,
        primary_node_arg_overrides,
        passthrough_global_args,
        cli_agent_override,
    } = request;

    let mut scheduled_template = template.clone();
    let primary_node = scheduled_template
        .nodes
        .iter_mut()
        .find(|node| node.id == primary_node_id)
        .ok_or_else(|| {
            format!(
                "workflow template selector `{}@{}` for scope `{}` is missing primary node `{}`",
                template_ref.id, template_ref.version, scope_label, primary_node_id
            )
        })?;
    primary_node.args.extend(primary_node_arg_overrides.clone());
    validate_template_agent_backends(
        &scheduled_template,
        &config::get_config(),
        cli_agent_override,
    )?;

    let node_schedule = compile_template_node_schedule(&scheduled_template)?;
    let node_lookup = scheduled_template
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<BTreeMap<_, _>>();
    let base_scope = base_metadata
        .command_alias
        .clone()
        .or(base_metadata.scope.clone())
        .unwrap_or_else(|| scope_label.clone());
    let builtin_selector = compat_scope
        .map(|scope| is_builtin_scope_selector(scope, template_ref))
        .unwrap_or(false);

    let mut planned_job_ids = BTreeMap::new();
    let mut used_job_ids = BTreeSet::new();
    used_job_ids.insert(root_job_id.to_string());
    for node_id in &node_schedule.order {
        let node_job_id = if node_id == primary_node_id {
            root_job_id.to_string()
        } else {
            let mut candidate = generate_job_id();
            while used_job_ids.contains(&candidate) {
                candidate = generate_job_id();
            }
            candidate
        };
        used_job_ids.insert(node_job_id.clone());
        planned_job_ids.insert(node_id.clone(), node_job_id);
    }

    let mut compiled_nodes = BTreeMap::new();
    for node_id in &node_schedule.order {
        let compiled = compile_template_node(
            &scheduled_template,
            node_id,
            &planned_job_ids,
            pinned_head.clone(),
        )?;
        compiled_nodes.insert(node_id.clone(), compiled);
    }

    let mut seen_primary = false;
    for node_id in &node_schedule.order {
        let node = node_lookup
            .get(node_id)
            .copied()
            .ok_or_else(|| format!("workflow template missing scheduled node `{node_id}`"))?;
        let node_job_id = planned_job_ids
            .get(&node.id)
            .cloned()
            .ok_or_else(|| format!("missing planned job id for node `{}`", node.id))?;
        let compiled = compiled_nodes
            .remove(node_id)
            .ok_or_else(|| format!("missing compiled metadata for node `{}`", node.id))?;
        let mut schedule = compiled.schedule;
        if node.after.is_empty() {
            schedule.after = jobs::resolve_after_dependencies_for_enqueue(
                jobs_root,
                &node_job_id,
                requested_after,
            )?;
        }

        let mut runtime_node = node.clone();
        if builtin_selector
            && let Some(scope) = compat_scope
            && mark_builtin_control_node_as_compat_noop(scope, node, primary_node_id)
        {
            runtime_node
                .args
                .insert("__compat_noop".to_string(), "true".to_string());
        }

        let raw_args = hidden_workflow_node_raw_args(
            &scope_label,
            &runtime_node,
            base_metadata,
            passthrough_global_args,
        )?;
        let follow_for_node = node.id == primary_node_id && follow_primary;

        let child_args =
            build_background_child_args(&raw_args, &node_job_id, background, follow_for_node, &[]);
        let recorded_args = user_friendly_args(&raw_args);

        let mut metadata = base_metadata.clone();
        if node.id != primary_node_id {
            metadata.scope = Some(wrapper_template_node_scope(&base_scope, node));
        }
        if metadata.workflow_template_selector.is_none() {
            metadata.workflow_template_selector = Some(format!(
                "{}@{}",
                compiled.template_id, compiled.template_version
            ));
        }
        metadata.workflow_template_id = Some(compiled.template_id);
        metadata.workflow_template_version = Some(compiled.template_version);
        metadata.workflow_node_id = Some(compiled.node_id);
        metadata.workflow_capability_id = compiled.capability_id;
        metadata.workflow_policy_snapshot_hash = Some(compiled.policy_snapshot_hash);
        metadata.workflow_gates = if compiled.gate_labels.is_empty() {
            None
        } else {
            Some(compiled.gate_labels)
        };

        jobs::enqueue_job(
            project_root,
            jobs_root,
            &node_job_id,
            &child_args,
            &recorded_args,
            Some(metadata),
            Some(config_snapshot.clone()),
            Some(schedule),
        )?;

        if capture_save_patch_for_primary && node.id == primary_node_id {
            capture_save_input_patch(project_root, jobs_root, &node_job_id)?;
        }

        if node.id == primary_node_id {
            seen_primary = true;
        }
    }

    if !seen_primary {
        return Err(format!(
            "workflow template selector `{}@{}` for scope `{}` did not schedule primary node `{}`",
            template_ref.id, template_ref.version, scope_label, primary_node_id
        )
        .into());
    }

    Ok(())
}

fn mark_builtin_control_node_as_compat_noop(
    scope: TemplateScope,
    node: &WorkflowNode,
    primary_node_id: &str,
) -> bool {
    if node.id == primary_node_id {
        return false;
    }
    match scope {
        TemplateScope::Approve => {
            workflow_node_capability(node) == Some(WorkflowCapability::GateStopCondition)
        }
        TemplateScope::Merge => matches!(
            workflow_node_capability(node),
            Some(WorkflowCapability::GateConflictResolution)
                | Some(WorkflowCapability::GateCicd)
                | Some(WorkflowCapability::RemediationCicdAutoFix)
        ),
        _ => false,
    }
}

fn is_builtin_scope_selector(scope: TemplateScope, template_ref: &WorkflowTemplateRef) -> bool {
    if template_ref.source_path.is_some() {
        return false;
    }
    let expected_id = match scope {
        TemplateScope::Save => "template.save",
        TemplateScope::Draft => "template.draft",
        TemplateScope::Approve => "template.approve",
        TemplateScope::Review => "template.review",
        TemplateScope::Merge => "template.merge",
        TemplateScope::BuildExecute => "template.build_execute",
        TemplateScope::Patch => "template.patch",
    };
    template_ref.id == expected_id && template_ref.version == "v1"
}

fn wrapper_template_node_scope(base_scope: &str, node: &WorkflowNode) -> String {
    format!("{base_scope}_template_{:?}", node.kind).to_ascii_lowercase()
}

fn hidden_workflow_node_raw_args(
    scope_label: &str,
    node: &WorkflowNode,
    metadata: &jobs::JobMetadata,
    passthrough_global_args: &[String],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut raw = vec!["vizier".to_string()];
    raw.extend(passthrough_global_args.iter().cloned());
    raw.extend([
        "__workflow-node".to_string(),
        "--scope".to_string(),
        scope_label.to_string(),
        "--node".to_string(),
        node.id.clone(),
        "--node-json".to_string(),
        serde_json::to_string(node)?,
    ]);
    if let Some(plan) = metadata.plan.as_ref() {
        raw.push("--slug".to_string());
        raw.push(plan.clone());
    }
    if let Some(branch) = metadata.branch.as_ref() {
        raw.push("--branch".to_string());
        raw.push(branch.clone());
    }
    if let Some(target) = metadata.target.as_ref() {
        raw.push("--target".to_string());
        raw.push(target.clone());
    }
    Ok(raw)
}
