use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use vizier_core::{config, vcs};

use crate::actions::*;
use crate::cli::args::{
    ApproveCmd, CdCmd, CleanCmd, DraftCmd, GlobalOpts, ListCmd, MergeCmd, ResolvedInput, ReviewCmd,
    TestDisplayCmd,
};
use crate::plan;

fn read_all_stdin() -> Result<String, std::io::Error> {
    use std::io::Read;
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

pub(crate) fn resolve_draft_spec(
    cmd: &DraftCmd,
) -> Result<ResolvedInput, Box<dyn std::error::Error>> {
    resolve_prompt_input(cmd.spec.as_deref(), cmd.file.as_deref())
}

pub(crate) fn resolve_list_options(
    cmd: &ListCmd,
    emit_json: bool,
) -> Result<ListOptions, Box<dyn std::error::Error>> {
    let fields = if let Some(raw) = cmd.fields.as_ref() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("list fields cannot be empty".into());
        }
        let parsed = trimmed
            .split(',')
            .map(|field| field.trim())
            .filter(|field| !field.is_empty())
            .map(|field| field.to_string())
            .collect::<Vec<_>>();
        if parsed.is_empty() {
            return Err("list fields cannot be empty".into());
        }
        Some(parsed)
    } else {
        None
    };

    Ok(ListOptions {
        target: cmd.target.clone(),
        format: cmd.format.map(Into::into),
        fields,
        emit_json,
    })
}

pub(crate) fn resolve_cd_options(cmd: &CdCmd) -> Result<CdOptions, Box<dyn std::error::Error>> {
    let plan = cmd
        .plan
        .as_deref()
        .ok_or("plan argument is required for vizier cd")?;
    let slug = plan::sanitize_name_override(plan).map_err(|err| {
        Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
    })?;
    let branch = cmd
        .branch
        .clone()
        .unwrap_or_else(|| plan::default_branch_for_slug(&slug));

    Ok(CdOptions {
        slug,
        branch,
        path_only: cmd.path_only,
    })
}

pub(crate) fn resolve_clean_options(
    cmd: &CleanCmd,
) -> Result<CleanOptions, Box<dyn std::error::Error>> {
    let slug = if let Some(plan) = cmd.plan.as_deref() {
        Some(plan::sanitize_name_override(plan).map_err(|err| {
            Box::<dyn std::error::Error>::from(io::Error::new(io::ErrorKind::InvalidInput, err))
        })?)
    } else {
        None
    };

    Ok(CleanOptions {
        slug,
        assume_yes: cmd.assume_yes,
    })
}

pub(crate) fn resolve_approve_options(
    cmd: &ApproveCmd,
    push_after: bool,
) -> Result<ApproveOptions, Box<dyn std::error::Error>> {
    let config = config::get_config();
    let repo_root = vcs::repo_root().ok();

    let mut stop_script = config
        .approve
        .stop_condition
        .script
        .clone()
        .map(|path| resolve_cicd_script_path(&path, repo_root.as_deref()));

    if let Some(script) = cmd.stop_condition_script.as_ref() {
        stop_script = Some(resolve_cicd_script_path(script, repo_root.as_deref()));
    }

    if let Some(script) = stop_script.as_ref() {
        let metadata = std::fs::metadata(script).map_err(|err| {
            format!(
                "unable to read approve stop-condition script {}: {err}",
                script.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "approve stop-condition script {} must be a file",
                script.display()
            )
            .into());
        }
    }

    let mut stop_retries = config.approve.stop_condition.retries;
    if let Some(retries) = cmd.stop_condition_retries {
        stop_retries = retries;
    }

    Ok(ApproveOptions {
        plan: cmd.plan.clone(),
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        stop_condition: ApproveStopCondition {
            script: stop_script,
            retries: stop_retries,
        },
        push_after,
    })
}

pub(crate) fn resolve_review_options(
    cmd: &ReviewCmd,
    push_after: bool,
) -> Result<ReviewOptions, Box<dyn std::error::Error>> {
    let plan = cmd
        .plan
        .clone()
        .ok_or("plan argument is required for vizier review")?;
    let config = config::get_config();
    let repo_root = vcs::repo_root().ok();
    let mut cicd_gate = CicdGateOptions::from_config(&config.merge.cicd_gate);
    let mut auto_resolve_requested = cicd_gate.auto_resolve;
    if let Some(script) = cicd_gate.script.clone() {
        cicd_gate.script = Some(resolve_cicd_script_path(&script, repo_root.as_deref()));
    }
    if let Some(script) = cmd.cicd_script.as_ref() {
        cicd_gate.script = Some(resolve_cicd_script_path(script, repo_root.as_deref()));
    }
    if cmd.auto_cicd_fix {
        auto_resolve_requested = true;
    }
    if cmd.no_auto_cicd_fix {
        auto_resolve_requested = false;
    }
    if let Some(retries) = cmd.cicd_retries {
        cicd_gate.retries = retries;
    }
    // Review runs the gate without auto-remediation unless explicitly allowed in the future.
    cicd_gate.auto_resolve = false;
    if let Some(script) = cicd_gate.script.as_ref() {
        let metadata = std::fs::metadata(script)
            .map_err(|err| format!("unable to read CI/CD script {}: {err}", script.display()))?;
        if !metadata.is_file() {
            return Err(format!("CI/CD script {} must be a file", script.display()).into());
        }
    }

    Ok(ReviewOptions {
        plan,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        review_only: cmd.review_only,
        review_file: cmd.review_file,
        skip_checks: cmd.skip_checks,
        cicd_gate,
        auto_resolve_requested,
        push_after,
    })
}

pub(crate) fn resolve_merge_options(
    cmd: &MergeCmd,
    push_after: bool,
) -> Result<MergeOptions, Box<dyn std::error::Error>> {
    let plan = cmd
        .plan
        .clone()
        .ok_or("plan argument is required for vizier merge")?;
    let config = config::get_config();
    let default_conflict_auto_resolve = config::MergeConflictsConfig::default().auto_resolve;
    let conflict_source = if config.merge.conflicts.auto_resolve == default_conflict_auto_resolve {
        ConflictAutoResolveSource::Default
    } else {
        ConflictAutoResolveSource::Config
    };
    let mut conflict_auto_resolve =
        ConflictAutoResolveSetting::new(config.merge.conflicts.auto_resolve, conflict_source);
    if cmd.auto_resolve_conflicts {
        conflict_auto_resolve =
            ConflictAutoResolveSetting::new(true, ConflictAutoResolveSource::FlagEnable);
    }
    if cmd.no_auto_resolve_conflicts {
        conflict_auto_resolve =
            ConflictAutoResolveSetting::new(false, ConflictAutoResolveSource::FlagDisable);
    }
    let conflict_strategy = if conflict_auto_resolve.enabled() {
        MergeConflictStrategy::Agent
    } else {
        MergeConflictStrategy::Manual
    };

    let repo_root = vcs::repo_root().ok();
    let mut cicd_gate = CicdGateOptions::from_config(&config.merge.cicd_gate);
    if let Some(script) = cicd_gate.script.clone() {
        cicd_gate.script = Some(resolve_cicd_script_path(&script, repo_root.as_deref()));
    }
    if let Some(script) = cmd.cicd_script.as_ref() {
        cicd_gate.script = Some(resolve_cicd_script_path(script, repo_root.as_deref()));
    }
    if cmd.auto_cicd_fix {
        cicd_gate.auto_resolve = true;
    }
    if cmd.no_auto_cicd_fix {
        cicd_gate.auto_resolve = false;
    }
    if let Some(retries) = cmd.cicd_retries {
        cicd_gate.retries = retries;
    }
    if let Some(script) = cicd_gate.script.as_ref() {
        let metadata = std::fs::metadata(script)
            .map_err(|err| format!("unable to read CI/CD script {}: {err}", script.display()))?;
        if !metadata.is_file() {
            return Err(format!("CI/CD script {} must be a file", script.display()).into());
        }
    }

    let mut squash = config.merge.squash_default;
    if cmd.squash {
        squash = true;
    }
    if cmd.no_squash {
        squash = false;
    }
    let mut squash_mainline = config.merge.squash_mainline;
    if let Some(mainline) = cmd.squash_mainline {
        squash_mainline = Some(mainline);
    }
    if let Some(mainline) = squash_mainline
        && mainline == 0
    {
        return Err("squash mainline parent index must be at least 1".into());
    }

    Ok(MergeOptions {
        plan,
        target: cmd.target.clone(),
        branch_override: cmd.branch.clone(),
        assume_yes: cmd.assume_yes,
        delete_branch: !cmd.keep_branch,
        note: cmd.note.clone(),
        push_after,
        conflict_auto_resolve,
        conflict_strategy,
        complete_conflict: cmd.complete_conflict,
        cicd_gate,
        squash,
        squash_mainline,
    })
}

pub(crate) fn resolve_test_display_options(
    cmd: &TestDisplayCmd,
) -> Result<TestDisplayOptions, Box<dyn std::error::Error>> {
    if let Some(prompt) = cmd
        .prompt
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| value.is_empty())
    {
        return Err(format!("prompt override cannot be empty (got {prompt:?})").into());
    }

    let command_alias = if let Some(scope_arg) = cmd.scope {
        let scope: config::CommandScope = scope_arg.into();
        config::CommandAlias::from(scope)
    } else {
        cmd.command.parse::<config::CommandAlias>().map_err(|err| {
            Box::<dyn std::error::Error>::from(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid --command alias `{}`: {err}", cmd.command),
            ))
        })?
    };

    Ok(TestDisplayOptions {
        command_alias,
        prompt_override: cmd.prompt.as_ref().map(|value| value.trim().to_string()),
        raw_output: cmd.raw,
        timeout: cmd.timeout_secs.map(std::time::Duration::from_secs),
        disable_wrapper: cmd.no_wrapper,
        record_session: cmd.session && !cmd.no_session,
    })
}

pub(crate) fn build_cli_agent_overrides(
    opts: &GlobalOpts,
) -> Result<Option<config::AgentOverrides>, Box<dyn std::error::Error>> {
    let mut overrides = config::AgentOverrides::default();

    if let Some(agent) = opts.agent.as_ref()
        && !agent.trim().is_empty()
    {
        overrides.selector = Some(agent.trim().to_ascii_lowercase());
    }

    if let Some(label) = opts
        .agent_label
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        overrides
            .agent_runtime
            .get_or_insert_with(Default::default)
            .label = Some(label.to_ascii_lowercase());
    }

    if let Some(command) = opts
        .agent_command
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        overrides
            .agent_runtime
            .get_or_insert_with(Default::default)
            .command = Some(vec![command.to_string()]);
    }

    if overrides.is_empty() {
        Ok(None)
    } else {
        Ok(Some(overrides))
    }
}

pub(crate) fn resolve_prompt_input(
    positional: Option<&str>,
    file: Option<&Path>,
) -> Result<ResolvedInput, Box<dyn std::error::Error>> {
    use std::io::{Error, ErrorKind};

    if positional.is_some() && file.is_some() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "cannot provide both MESSAGE and --file; choose one input source",
        )
        .into());
    }

    if let Some(path) = file {
        let msg = std::fs::read_to_string(path).map_err(|err| {
            Error::new(
                err.kind(),
                format!("failed to read {}: {err}", path.display()),
            )
        })?;

        if msg.trim().is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "file {} is empty; provide non-empty content",
                    path.display()
                ),
            )
            .into());
        }

        return Ok(ResolvedInput {
            text: msg,
            origin: crate::cli::args::InputOrigin::File(path.to_path_buf()),
        });
    }

    match positional {
        Some("-") => {
            // Explicit “read stdin”
            let msg = read_all_stdin()?;
            if msg.trim().is_empty() {
                return Err("stdin is empty; provide MESSAGE or pipe content".into());
            }
            Ok(ResolvedInput {
                text: msg,
                origin: crate::cli::args::InputOrigin::Stdin,
            })
        }
        Some(positional) => Ok(ResolvedInput {
            text: positional.to_owned(),
            origin: crate::cli::args::InputOrigin::Inline,
        }),
        None => {
            // No positional; try stdin if it’s not a TTY (i.e., piped or redirected)
            if !io::stdin().is_terminal() {
                let msg = read_all_stdin()?;
                if msg.trim().is_empty() {
                    return Err("stdin is empty; provide MESSAGE or pipe content".into());
                }
                Ok(ResolvedInput {
                    text: msg,
                    origin: crate::cli::args::InputOrigin::Stdin,
                })
            } else {
                Err("no MESSAGE provided; pass a message, use '-', or pipe stdin".into())
            }
        }
    }
}

fn resolve_cicd_script_path(script: &Path, repo_root: Option<&Path>) -> PathBuf {
    if script.is_absolute() {
        return script.to_path_buf();
    }
    if let Some(root) = repo_root {
        return root.join(script);
    }
    script.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::resolve_prompt_input;
    use std::io::Write;

    #[test]
    fn resolve_prompt_input_reads_file_contents() -> Result<(), Box<dyn std::error::Error>> {
        let mut tmp = tempfile::NamedTempFile::new()?;
        write!(tmp, "File-backed prompt")?;

        let resolved = resolve_prompt_input(None, Some(tmp.path()))?;
        assert_eq!(resolved.text, "File-backed prompt");
        Ok(())
    }

    #[test]
    fn resolve_prompt_input_rejects_both_sources() {
        let err = resolve_prompt_input(Some("inline"), Some(std::path::Path::new("ignored")))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot provide both MESSAGE and --file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_prompt_input_rejects_empty_file() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::NamedTempFile::new()?;

        let err = resolve_prompt_input(None, Some(tmp.path()))
            .expect_err("empty file should produce an error");
        assert!(err.to_string().contains("empty"), "unexpected error: {err}");
        Ok(())
    }
}
