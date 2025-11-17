use std::io::{self, Write};

use clap::builder::StyledStr;
use clap::Command;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use clap_complete::env::Shells;
use clap_complete::CompleteEnv;
use clap_complete::Shell;

use crate::plan::{PlanSlugEntry, PlanSlugInventory};

const COMPLETION_ENV_VAR: &str = "COMPLETE";

pub fn try_handle_completion(
    factory: impl Fn() -> Command,
) -> clap::error::Result<bool> {
    CompleteEnv::with_factory(factory)
        .try_complete(std::env::args_os(), std::env::current_dir().ok().as_deref())
}

pub fn write_registration(
    shell: Shell,
    factory: impl Fn() -> Command,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = factory();
    cmd.build();

    let shells = Shells::builtins();
    let shell_name = match shell {
        Shell::Bash => "bash",
        Shell::Zsh => "zsh",
        Shell::Fish => "fish",
        Shell::Elvish => "elvish",
        Shell::PowerShell => "powershell",
        other => {
            return Err(format!("unsupported shell {other:?}").into());
        }
    };

    let completer = shells
        .completer(shell_name)
        .ok_or_else(|| format!("unsupported shell {shell_name}"))?;

    let bin = cmd
        .get_bin_name()
        .unwrap_or_else(|| cmd.get_name())
        .to_string();

    let completer_path = std::env::args()
        .next()
        .unwrap_or_else(|| bin.clone());

    let mut buf = Vec::new();
    completer.write_registration(
        COMPLETION_ENV_VAR,
        cmd.get_name(),
        &bin,
        &completer_path,
        &mut buf,
    )?;
    io::stdout().write_all(&buf)?;
    Ok(())
}

pub fn plan_slug_completer() -> ArgValueCompleter {
    ArgValueCompleter::new(|current: &std::ffi::OsStr| {
        let prefix = current.to_string_lossy().to_string();
        let entries = PlanSlugInventory::collect(None).unwrap_or_default();
        entries
            .into_iter()
            .filter(|entry| entry.slug.starts_with(prefix.as_str()))
            .map(candidate_for_entry)
            .collect()
    })
}

fn candidate_for_entry(entry: PlanSlugEntry) -> CompletionCandidate {
    CompletionCandidate::new(entry.slug).help(Some(StyledStr::from(entry.summary)))
}
