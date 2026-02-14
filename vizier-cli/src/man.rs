use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ColorChoice;

use crate::cli::help::{render_clap_help_text, render_clap_subcommand_help_text, strip_ansi_codes};

const MAN1_DIR: &str = "docs/man/man1";
const VIZIER_MAN1_PATH: &str = "docs/man/man1/vizier.1";
const VIZIER_JOBS_MAN1_PATH: &str = "docs/man/man1/vizier-jobs.1";

#[derive(Debug, Clone)]
struct PageOutput {
    path: PathBuf,
    content: String,
}

pub fn generate_man_pages(check: bool) -> Result<(), Box<dyn Error>> {
    let root_help = strip_ansi_codes(&render_clap_help_text(ColorChoice::Never));
    let jobs_help = strip_ansi_codes(&render_clap_subcommand_help_text(
        ColorChoice::Never,
        "jobs",
    )?);

    let outputs = vec![
        PageOutput {
            path: PathBuf::from(VIZIER_MAN1_PATH),
            content: render_root_page(&root_help),
        },
        PageOutput {
            path: PathBuf::from(VIZIER_JOBS_MAN1_PATH),
            content: render_jobs_page(&jobs_help),
        },
    ];

    if check {
        let mut drift = Vec::new();
        for output in &outputs {
            match fs::read_to_string(&output.path) {
                Ok(existing) if existing == output.content => {}
                Ok(_) | Err(_) => drift.push(output.path.display().to_string()),
            }
        }
        if drift.is_empty() {
            return Ok(());
        }
        return Err(format!(
            "generated man pages are stale; run `cargo run -p vizier --bin gen-man --`:\n{}",
            drift.join("\n")
        )
        .into());
    }

    fs::create_dir_all(Path::new(MAN1_DIR))?;
    for output in outputs {
        write_if_changed(&output.path, &output.content)?;
    }
    Ok(())
}

fn render_root_page(help: &str) -> String {
    let sections = [("REFERENCE", help.to_string())];
    render_command_page(
        "vizier",
        "Vizier command-line interface",
        &sections,
        &[
            ("vizier-jobs", 1),
            ("vizier-config", 5),
            ("vizier-workflow", 7),
        ],
    )
}

fn render_jobs_page(help: &str) -> String {
    let sections = [("REFERENCE", help.to_string())];
    render_command_page(
        "vizier-jobs",
        "scheduler and job operations",
        &sections,
        &[("vizier", 1), ("vizier-workflow", 7)],
    )
}

fn render_command_page(
    name: &str,
    description: &str,
    sections: &[(&str, String)],
    see_also: &[(&str, u8)],
) -> String {
    let mut output = format!(
        ".TH {} 1 \"UNRELEASED\" \"Vizier\" \"User Commands\"\n",
        name.to_ascii_uppercase()
    );
    output.push_str(".SH NAME\n");
    output.push_str(name);
    output.push_str(" \\- ");
    output.push_str(description);
    output.push('\n');

    for (title, body) in sections {
        output.push_str(".SH ");
        output.push_str(title);
        output.push('\n');
        output.push_str(".nf\n");
        append_escaped_roff(&mut output, body);
        output.push_str(".fi\n");
    }

    output.push_str(".SH SEE ALSO\n");
    for (idx, (other, section)) in see_also.iter().enumerate() {
        if idx > 0 {
            output.push_str(",\n");
        }
        output.push_str(".BR ");
        output.push_str(other);
        output.push_str(" (");
        output.push_str(&section.to_string());
        output.push(')');
    }
    output.push('\n');

    output
}

fn append_escaped_roff(target: &mut String, text: &str) {
    for line in text.split_inclusive('\n') {
        if line.starts_with('.') || line.starts_with('\'') {
            target.push_str("\\&");
        }
        target.push_str(line);
    }
}

fn write_if_changed(path: &Path, content: &str) -> Result<(), Box<dyn Error>> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            fs::remove_file(path)?;
        } else if fs::read_to_string(path).ok().as_deref() == Some(content) {
            return Ok(());
        }
    }
    fs::write(path, content)?;
    Ok(())
}
