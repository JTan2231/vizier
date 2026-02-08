use std::io::{self, IsTerminal, Write};

use clap::{ColorChoice, CommandFactory, FromArgMatches, error::ErrorKind};
use vizier_core::{
    auditor, config,
    display::{self, LogLevel},
    tools, vcs,
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
    scheduler_supported, strip_stdin_marker, user_friendly_args,
};
use crate::cli::util::flag_present;
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
        return Err("`ask` has been removed; use supported workflow commands (`save`, `draft`, `approve`, `review`, `merge`).".into());
    }
    let quiet_requested = flag_present(&raw_args, Some('q'), "--quiet");
    let json_requested = flag_present(&raw_args, Some('j'), "--json");
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

                render_help_with_pager(
                    &help_text,
                    pager_mode,
                    stdout_is_tty,
                    quiet_requested || json_requested,
                )?;
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

        render_help_with_pager(
            &help_text,
            pager_mode,
            stdout_is_tty,
            quiet_requested || json_requested,
        )?;
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
        print_json: cli.global.json,
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

    if let Some(label) = cli
        .global
        .agent_label
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        cfg.agent_runtime.label = Some(label.to_ascii_lowercase());
    }

    if let Some(command) = cli
        .global
        .agent_command
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        cfg.agent_runtime.command = vec![command.to_string()];
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
        if cli.global.no_background {
            return Err(
                "foreground execution is no longer supported; scheduled commands always run as jobs"
                    .into(),
            );
        }

        if cli.global.json {
            return Err("--json cannot be used with scheduled execution; use `vizier jobs show --format json` instead".into());
        }

        let job_id = generate_job_id();
        let mut raw_args_for_child = raw_args.clone();
        let mut injected_args: Vec<String> = Vec::new();
        let mut schedule = jobs::JobSchedule::default();
        let mut capture_save_patch = false;
        let requested_after = match &cli.command {
            Commands::Save(cmd) => cmd.after.clone(),
            Commands::Draft(cmd) => cmd.after.clone(),
            Commands::Patch(cmd) => cmd.after.clone(),
            Commands::Approve(cmd) => cmd.after.clone(),
            Commands::Review(cmd) => cmd.after.clone(),
            Commands::Merge(cmd) => cmd.after.clone(),
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
                schedule.pinned_head = Some(pinned.clone());
                schedule.locks = vec![
                    jobs::JobLock {
                        key: "repo_serial".to_string(),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("branch:{}", pinned.branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("temp_worktree:{job_id}"),
                        mode: jobs::LockMode::Exclusive,
                    },
                ];
                schedule.artifacts = vec![jobs::JobArtifact::CommandPatch {
                    job_id: job_id.clone(),
                }];
                metadata.target = Some(pinned.branch);
                capture_save_patch = true;
            }
            Commands::Draft(cmd) => {
                let (resolved, input_file) = prepare_prompt_input(
                    cmd.spec.as_deref(),
                    cmd.file.as_deref(),
                    &project_root,
                    &job_id,
                )?;
                if let Some(path) = input_file {
                    raw_args_for_child = strip_stdin_marker(&raw_args_for_child);
                    injected_args.push("--file".to_string());
                    injected_args.push(path.display().to_string());
                }
                let plan_dir = project_root.join(".vizier/implementation-plans");
                std::fs::create_dir_all(&plan_dir)?;
                let base_slug = if let Some(name) = cmd.name.as_ref() {
                    plan::sanitize_name_override(name)?
                } else {
                    plan::slug_from_spec(&resolved.text)
                };
                let slug = plan::ensure_unique_slug(&base_slug, &plan_dir, "draft/")?;
                if cmd.name.is_none() {
                    injected_args.push("--name".to_string());
                    injected_args.push(slug.clone());
                }
                let branch = plan::default_branch_for_slug(&slug);
                schedule.locks = vec![
                    jobs::JobLock {
                        key: format!("branch:{branch}"),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("temp_worktree:{job_id}"),
                        mode: jobs::LockMode::Exclusive,
                    },
                ];
                schedule.artifacts = vec![
                    jobs::JobArtifact::PlanBranch {
                        slug: slug.clone(),
                        branch: branch.clone(),
                    },
                    jobs::JobArtifact::PlanDoc {
                        slug: slug.clone(),
                        branch: branch.clone(),
                    },
                ];
                metadata.plan = Some(slug);
                metadata.branch = Some(branch);
            }
            Commands::Patch(cmd) => {
                if !cmd.assume_yes && !io::stdin().is_terminal() {
                    return Err("vizier patch requires --yes in scheduler mode".into());
                }
                if !cmd.assume_yes {
                    let pipeline = match cmd.pipeline {
                        Some(BuildPipelineArg::Approve) => "approve",
                        Some(BuildPipelineArg::ApproveReview) => "approve-review",
                        Some(BuildPipelineArg::ApproveReviewMerge) => "approve-review-merge",
                        None => "approve-review-merge",
                    };
                    let confirmed = prompt_yes_no(&format!(
                        "Queue patch run for {} file(s) with pipeline {}?",
                        cmd.files.len(),
                        pipeline
                    ))?;
                    if !confirmed {
                        return Err("aborted by user".into());
                    }
                    injected_args.push("--yes".to_string());
                }
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
                    injected_args.push("--yes".to_string());
                }
                schedule.dependencies = vec![jobs::JobDependency {
                    artifact: jobs::JobArtifact::PlanDoc {
                        slug: spec.slug.clone(),
                        branch: spec.branch.clone(),
                    },
                }];
                schedule.locks = vec![
                    jobs::JobLock {
                        key: format!("branch:{}", spec.branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("temp_worktree:{job_id}"),
                        mode: jobs::LockMode::Exclusive,
                    },
                ];
                schedule.artifacts = vec![jobs::JobArtifact::PlanCommits {
                    slug: spec.slug.clone(),
                    branch: spec.branch.clone(),
                }];
                if cmd.require_approval && !cmd.no_require_approval {
                    schedule.approval = Some(jobs::pending_job_approval());
                }
                metadata.plan = Some(spec.slug.clone());
                metadata.branch = Some(spec.branch.clone());
                metadata.target = Some(spec.target_branch.clone());
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
                if !cmd.assume_yes && !cmd.review_only && !cmd.review_file {
                    match prompt_review_queue_choice()? {
                        ReviewQueueChoice::ApplyFixes => {
                            injected_args.push("--yes".to_string());
                        }
                        ReviewQueueChoice::CritiqueOnly => {
                            injected_args.push("--review-only".to_string());
                        }
                        ReviewQueueChoice::ReviewFile => {
                            injected_args.push("--review-file".to_string());
                        }
                        ReviewQueueChoice::Cancel => {
                            return Err("aborted by user".into());
                        }
                    }
                }
                schedule.dependencies = vec![
                    jobs::JobDependency {
                        artifact: jobs::JobArtifact::PlanBranch {
                            slug: spec.slug.clone(),
                            branch: spec.branch.clone(),
                        },
                    },
                    jobs::JobDependency {
                        artifact: jobs::JobArtifact::PlanDoc {
                            slug: spec.slug.clone(),
                            branch: spec.branch.clone(),
                        },
                    },
                ];
                schedule.locks = vec![
                    jobs::JobLock {
                        key: format!("branch:{}", spec.branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("temp_worktree:{job_id}"),
                        mode: jobs::LockMode::Exclusive,
                    },
                ];
                schedule.artifacts = vec![jobs::JobArtifact::PlanCommits {
                    slug: spec.slug.clone(),
                    branch: spec.branch.clone(),
                }];
                metadata.plan = Some(spec.slug.clone());
                metadata.branch = Some(spec.branch.clone());
                metadata.target = Some(spec.target_branch.clone());
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
                    injected_args.push("--yes".to_string());
                }
                schedule.dependencies = vec![jobs::JobDependency {
                    artifact: jobs::JobArtifact::PlanBranch {
                        slug: spec.slug.clone(),
                        branch: spec.branch.clone(),
                    },
                }];
                schedule.locks = vec![
                    jobs::JobLock {
                        key: format!("branch:{}", spec.target_branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("branch:{}", spec.branch),
                        mode: jobs::LockMode::Exclusive,
                    },
                    jobs::JobLock {
                        key: format!("merge_sentinel:{}", spec.slug),
                        mode: jobs::LockMode::Exclusive,
                    },
                ];
                schedule.artifacts = vec![jobs::JobArtifact::TargetBranch {
                    name: spec.target_branch.clone(),
                }];
                metadata.plan = Some(spec.slug.clone());
                metadata.branch = Some(spec.branch.clone());
                metadata.target = Some(spec.target_branch.clone());
            }
            _ => {}
        }

        schedule.after =
            jobs::resolve_after_dependencies_for_enqueue(&jobs_root, &job_id, &requested_after)?;

        let child_args = build_background_child_args(
            &raw_args_for_child,
            &job_id,
            &workflow_defaults.background,
            follow,
            &injected_args,
        );
        let mut recorded_args = user_friendly_args(&raw_args_for_child);
        recorded_args.extend(injected_args.clone());
        let config_snapshot = background_config_snapshot(&config::get_config());

        jobs::enqueue_job(
            &project_root,
            &jobs_root,
            &job_id,
            &child_args,
            &recorded_args,
            Some(metadata),
            Some(config_snapshot),
            Some(schedule),
        )?;
        if capture_save_patch {
            capture_save_input_patch(&project_root, &jobs_root, &job_id)?;
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
            Commands::Completions(cmd) => {
                crate::completions::write_registration(cmd.shell.into(), Cli::command)?;
                Ok(())
            }
            Commands::Complete(_) => Ok(()),

            Commands::Save(SaveCmd {
                rev_or_range,
                commit_message,
                commit_message_editor,
                after: _,
            }) => {
                let agent = config::resolve_agent_settings(
                    &config::get_config(),
                    config::CommandScope::Save,
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
                let agent =
                    config::resolve_agent_settings(&cfg, opts.scope, cli_agent_override.as_ref())?;
                run_test_display(opts, &agent).await
            }

            Commands::Draft(cmd) => {
                let resolved = resolve_draft_spec(&cmd)?;
                let agent = config::resolve_agent_settings(
                    &config::get_config(),
                    config::CommandScope::Draft,
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
                if cli.global.json {
                    return Err("--json is not supported for vizier build".into());
                }
                match command {
                    Some(BuildActionCmd::Execute(exec)) => {
                        let pipeline = exec.pipeline.map(|value| match value {
                            BuildPipelineArg::Approve => BuildExecutionPipeline::Approve,
                            BuildPipelineArg::ApproveReview => {
                                BuildExecutionPipeline::ApproveReview
                            }
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
                    None => {
                        let build_file =
                            file.ok_or("vizier build requires --file when no subcommand is used")?;
                        let agent = config::resolve_agent_settings(
                            &config::get_config(),
                            config::CommandScope::Draft,
                            cli_agent_override.as_ref(),
                        )?;
                        run_build(build_file, name, &project_root, &agent, commit_mode).await
                    }
                }
            }

            Commands::Patch(cmd) => {
                if cli.global.json {
                    return Err("--json is not supported for vizier patch".into());
                }
                let pipeline = cmd.pipeline.map(|value| match value {
                    BuildPipelineArg::Approve => BuildExecutionPipeline::Approve,
                    BuildPipelineArg::ApproveReview => BuildExecutionPipeline::ApproveReview,
                    BuildPipelineArg::ApproveReviewMerge => {
                        BuildExecutionPipeline::ApproveReviewMerge
                    }
                });
                let agent = config::resolve_agent_settings(
                    &config::get_config(),
                    config::CommandScope::Draft,
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

            Commands::List(cmd) => run_list(resolve_list_options(&cmd, cli.global.json)?),
            Commands::Cd(cmd) => run_cd(resolve_cd_options(&cmd)?),
            Commands::Clean(cmd) => run_clean(resolve_clean_options(&cmd)?),
            Commands::Plan(_) => run_plan_summary(cli_agent_override.as_ref(), cli.global.json),
            Commands::Jobs(cmd) => run_jobs_command(
                &project_root,
                &jobs_root,
                cmd,
                cli.global.follow,
                cli.global.json,
            ),

            Commands::Approve(cmd) => {
                let opts = resolve_approve_options(&cmd, push_after)?;
                let agent = config::resolve_agent_settings(
                    &config::get_config(),
                    config::CommandScope::Approve,
                    cli_agent_override.as_ref(),
                )?;
                run_approve(opts, &agent, commit_mode).await
            }
            Commands::Review(cmd) => {
                let opts = resolve_review_options(&cmd, push_after)?;
                let agent = config::resolve_agent_settings(
                    &config::get_config(),
                    config::CommandScope::Review,
                    cli_agent_override.as_ref(),
                )?;
                run_review(opts, &agent, commit_mode).await
            }
            Commands::Merge(cmd) => {
                let opts = resolve_merge_options(&cmd, push_after)?;
                let agent = config::resolve_agent_settings(
                    &config::get_config(),
                    config::CommandScope::Merge,
                    cli_agent_override.as_ref(),
                )?;
                run_merge(opts, &agent, commit_mode).await
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
            if auditor_cleanup.print_json
                && let Ok(contents) = std::fs::read_to_string(&artifact.path)
            {
                println!("{contents}");
            }
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
