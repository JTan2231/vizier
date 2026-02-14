use std::io;

use crate::actions::{CdOptions, CleanOptions, ListOptions};
use crate::cli::args::{CdCmd, CleanCmd, ListCmd};
use crate::plan;

pub(crate) fn resolve_list_options(
    cmd: &ListCmd,
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
