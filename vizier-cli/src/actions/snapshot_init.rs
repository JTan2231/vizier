use vizier_core::{
    bootstrap,
    bootstrap::{BootstrapOptions, IssuesProvider},
    display::{self, Verbosity, format_number},
};

use super::shared::{append_agent_rows, current_verbosity, format_block, short_hash};
use super::types::SnapshotInitOptions;

pub(crate) async fn run_snapshot_init(
    opts: SnapshotInitOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let depth_preview = bootstrap::preview_history_depth(opts.depth)?;

    display::info(format!(
        "Bootstrapping snapshot (history depth target: {})",
        depth_preview
    ));
    if !opts.paths.is_empty() {
        display::info(format!("Scope includes: {}", opts.paths.join(", ")));
    }
    if !opts.exclude.is_empty() {
        display::info(format!("Scope excludes: {}", opts.exclude.join(", ")));
    }

    let issues_provider = if let Some(provider) = opts.issues {
        Some(provider.parse::<IssuesProvider>()?)
    } else {
        None
    };

    let report = bootstrap::bootstrap_snapshot(BootstrapOptions {
        force: opts.force,
        depth: opts.depth,
        paths: opts.paths.clone(),
        exclude: opts.exclude.clone(),
        issues_provider,
    })
    .await?;

    if !report.warnings.is_empty() {
        for note in &report.warnings {
            display::warn(format!("Warning: {}", note));
        }
    }

    let verbosity = current_verbosity();
    if !report.summary.trim().is_empty() {
        display::info(format!("Snapshot summary: {}", report.summary.trim()));
    }

    let files_updated = report.files_touched.len();
    let mut rows = vec![(
        "Outcome".to_string(),
        "Snapshot bootstrap complete".to_string(),
    )];
    rows.push(("Depth used".to_string(), format_number(report.depth_used)));
    rows.push((
        "Files".to_string(),
        if files_updated == 0 {
            "no .vizier changes".to_string()
        } else {
            format!("updated {}", format_number(files_updated))
        },
    ));

    if matches!(verbosity, Verbosity::Info | Verbosity::Debug) {
        rows.push(("Analyzed at".to_string(), report.analysis_timestamp.clone()));
        rows.push((
            "Branch".to_string(),
            report
                .branch
                .as_deref()
                .unwrap_or("<detached HEAD>")
                .to_string(),
        ));
        rows.push((
            "Head".to_string(),
            report
                .head_commit
                .as_deref()
                .map(short_hash)
                .unwrap_or_else(|| "<no HEAD commit>".to_string()),
        ));
        rows.push((
            "Working tree".to_string(),
            if report.dirty { "dirty" } else { "clean" }.to_string(),
        ));
        if !report.scope_includes.is_empty() {
            rows.push(("Includes".to_string(), report.scope_includes.join(", ")));
        }
        if !report.scope_excludes.is_empty() {
            rows.push(("Excludes".to_string(), report.scope_excludes.join(", ")));
        }
        if let Some(provider) = report.issues_provider.as_ref() {
            rows.push(("Issues provider".to_string(), provider.to_string()));
        }
        if !report.issues.is_empty() {
            rows.push(("Issues".to_string(), report.issues.join(", ")));
        }
    }

    append_agent_rows(&mut rows, verbosity);

    let outcome = format_block(rows);
    if !outcome.is_empty() {
        println!("{outcome}");
    }

    Ok(())
}
