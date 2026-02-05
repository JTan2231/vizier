use std::io::Write;
use std::process::{Command, Stdio};

use clap::{ColorChoice, CommandFactory, error::ErrorKind};

use super::args::Cli;
use super::util::flag_present;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PagerMode {
    Auto,
    Always,
    Never,
}

pub(crate) fn pager_mode_from_args(args: &[String]) -> PagerMode {
    if flag_present(args, None, "--no-pager") {
        PagerMode::Never
    } else if flag_present(args, None, "--pager") {
        PagerMode::Always
    } else {
        PagerMode::Auto
    }
}

pub(crate) fn render_help_with_pager(
    help_text: &str,
    pager_mode: PagerMode,
    stdout_is_tty: bool,
    suppress_pager: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if suppress_pager || !stdout_is_tty || matches!(pager_mode, PagerMode::Never) {
        print!("{help_text}");
        return Ok(());
    }

    if let Some(pager) = std::env::var("VIZIER_PAGER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        && try_page_output(&pager, help_text).is_ok()
    {
        return Ok(());
    }

    if matches!(pager_mode, PagerMode::Always | PagerMode::Auto)
        && try_page_output("less -FRSX", help_text).is_ok()
    {
        return Ok(());
    }

    print!("{help_text}");
    Ok(())
}

pub(crate) fn curated_help_text() -> &'static str {
    concat!(
        "Vizier — LLM-assisted plan workflow\n",
        "\n",
        "Workflow:\n",
        "  vizier draft  --file spec.md --name add-redis\n",
        "  vizier approve add-redis\n",
        "  vizier review  add-redis\n",
        "  vizier merge   add-redis\n",
        "\n",
        "Examples:\n",
        "  vizier build --file examples/build/todo.toml\n",
        "  vizier draft --name fix-help \"curate root help output\"\n",
        "  vizier merge fix-help --yes\n",
        "\n",
        "More help:\n",
        "  vizier help --all\n",
        "  vizier help <command>\n",
        "  man vizier\n",
        "\n",
    )
}

pub(crate) fn subcommand_from_raw_args(raw_args: &[String]) -> Option<String> {
    let mut iter = raw_args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }

        if let Some((flag, _value)) = arg.split_once('=')
            && global_arg_takes_value(flag)
        {
            continue;
        }

        if arg.starts_with('-') {
            if global_arg_takes_value(arg) {
                iter.next();
            }
            continue;
        }

        return Some(arg.to_string());
    }

    None
}

pub(crate) fn render_clap_help_text(color_choice: ColorChoice) -> String {
    Cli::command()
        .color(color_choice)
        .render_long_help()
        .to_string()
}

pub(crate) fn render_clap_subcommand_help_text(
    color_choice: ColorChoice,
    command: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let root = Cli::command().color(color_choice);
    let args = [
        "vizier".to_string(),
        command.to_string(),
        "--help".to_string(),
    ];
    match root.try_get_matches_from(args) {
        Ok(_) => Err(format!("expected `vizier {command} --help` to display help").into()),
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp => Ok(err.render().to_string()),
            _ => Err(Box::<dyn std::error::Error>::from(err)),
        },
    }
}

pub(crate) fn strip_ansi_codes(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
            continue;
        }
        output.push(ch);
    }
    output
}

fn global_arg_takes_value(arg: &str) -> bool {
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

fn try_page_output(command: &str, contents: &str) -> std::io::Result<()> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .spawn()?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(contents.as_bytes())?;
    }

    let _ = child.wait()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::subcommand_from_raw_args;

    #[test]
    fn subcommand_from_raw_args_skips_global_flag_with_equals() {
        let raw_args = vec![
            "vizier".to_string(),
            "--config-file=./vizier.toml".to_string(),
            "ask".to_string(),
        ];

        assert_eq!(subcommand_from_raw_args(&raw_args), Some("ask".to_string()));
    }
}
