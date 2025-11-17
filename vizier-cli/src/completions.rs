use std::io::{self, Write};

use clap::Command;
use clap::builder::StyledStr;
use clap_complete::CompleteEnv;
use clap_complete::Shell;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use clap_complete::env::Shells;

use crate::plan::{PlanSlugEntry, PlanSlugInventory};

const COMPLETION_ENV_VAR: &str = "COMPLETE";
pub const RUNTIME_SUBCOMMAND: &str = "__complete";

pub fn try_handle_completion(factory: impl Fn() -> Command) -> clap::error::Result<bool> {
    let completer = runtime_invocation();
    CompleteEnv::with_factory(factory)
        .completer(completer)
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

    let mut buf = Vec::new();
    completer.write_registration(
        COMPLETION_ENV_VAR,
        cmd.get_name(),
        &bin,
        &runtime_invocation(),
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

fn runtime_invocation() -> String {
    let bin = std::env::args()
        .next()
        .unwrap_or_else(|| "vizier".to_string());
    format!("{bin} {RUNTIME_SUBCOMMAND}")
}
