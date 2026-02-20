use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use clap::{ColorChoice, CommandFactory, error::ErrorKind};
use vizier_core::config;

use super::args::Cli;
use super::util::flag_present;
use crate::workflow_templates;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PagerMode {
    Auto,
    Never,
}

pub(crate) fn pager_mode_from_args(args: &[String]) -> PagerMode {
    if flag_present(args, None, "--no-pager") {
        PagerMode::Never
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

    if try_page_output("less -FRSX", help_text).is_ok() {
        return Ok(());
    }

    print!("{help_text}");
    Ok(())
}

pub(crate) fn curated_help_text() -> &'static str {
    concat!(
        "Vizier — repository maintenance CLI\n",
        "\n",
        "Core commands:\n",
        "  vizier init --check\n",
        "  vizier list\n",
        "  vizier jobs list\n",
        "  vizier run develop\n",
        "  vizier audit develop\n",
        "  vizier release --dry-run\n",
        "\n",
        "Examples:\n",
        "  vizier jobs schedule --watch\n",
        "  vizier jobs tail <job-id> --follow\n",
        "  vizier completions zsh\n",
        "  vizier help jobs\n",
        "\n",
        "More help:\n",
        "  vizier help --all\n",
        "  vizier help <command>\n",
        "  man vizier\n",
        "\n",
    )
}

pub(crate) fn render_run_workflow_help_text(
    project_root: &Path,
    flow: &str,
    cfg: &config::Config,
) -> Result<String, Box<dyn std::error::Error>> {
    let source = workflow_templates::resolve_workflow_source(project_root, flow, cfg)?;
    let input_spec = workflow_templates::load_template_input_spec(&source)?;
    let flow_label = source
        .command_alias
        .as_ref()
        .map(|alias| alias.as_str().to_string())
        .unwrap_or_else(|| source.selector.clone());

    let ordered_params = ordered_cli_params(&input_spec);
    let has_cli_metadata = !input_spec.positional.is_empty() || !input_spec.named.is_empty();

    let mut lines = Vec::<String>::new();
    lines.push(format!("Workflow: {flow_label}"));
    lines.push(format!("Source: {}", source.selector));
    lines.push(String::new());

    lines.push("Usage:".to_string());
    if has_cli_metadata {
        if !input_spec.positional.is_empty() {
            let placeholders = input_spec
                .positional
                .iter()
                .map(|param| {
                    format!(
                        "<{}>",
                        kebab_case_key(&cli_label_for_param(&input_spec, param))
                    )
                })
                .collect::<Vec<_>>();
            lines.push(format!(
                "  vizier run {flow_label} {}",
                placeholders.join(" ")
            ));
        }

        if !ordered_params.is_empty() {
            let flags = ordered_params
                .iter()
                .map(|param| {
                    let label = cli_label_for_param(&input_spec, param);
                    format!(
                        "[--{} <{}>]",
                        kebab_case_key(&label),
                        kebab_case_key(&label)
                    )
                })
                .collect::<Vec<_>>();
            lines.push(format!("  vizier run {flow_label} {}", flags.join(" ")));
        } else {
            lines.push(format!("  vizier run {flow_label}"));
        }
    } else {
        lines.push(format!("  vizier run {flow_label} [--set <KEY=VALUE>]..."));
    }
    lines.push(String::new());

    lines.push("Inputs:".to_string());
    if has_cli_metadata {
        if !input_spec.positional.is_empty() {
            lines.push("  Positional:".to_string());
            for (index, param) in input_spec.positional.iter().enumerate() {
                let label = cli_label_for_param(&input_spec, param);
                if label == *param {
                    lines.push(format!("    {}. <{}>", index + 1, kebab_case_key(&label)));
                } else {
                    lines.push(format!(
                        "    {}. <{}> -> {}",
                        index + 1,
                        kebab_case_key(&label),
                        param
                    ));
                }
            }
        }

        let named_entries = named_entries(&input_spec, &ordered_params);
        if !named_entries.is_empty() {
            lines.push("  Named:".to_string());
            for (alias, target) in named_entries {
                let flag = kebab_case_key(&alias);
                let placeholder = kebab_case_key(&alias);
                if alias == target {
                    lines.push(format!("    --{flag} <{placeholder}>"));
                } else {
                    lines.push(format!("    --{flag} <{placeholder}> -> {target}"));
                }
            }
        }
    } else {
        lines.push(
            "  No [cli] aliases are defined; pass parameters with --set key=value.".to_string(),
        );
        if !input_spec.params.is_empty() {
            lines.push(format!("  Known params: {}", input_spec.params.join(", ")));
        }
    }
    lines.push(String::new());

    lines.push("Examples:".to_string());
    if has_cli_metadata {
        lines.push(format!(
            "  {}",
            named_example(&flow_label, &input_spec, &ordered_params)
        ));
        if let Some(example) = positional_example(&flow_label, &input_spec) {
            lines.push(format!("  {example}"));
        }
    } else if let Some(param) = input_spec.params.first() {
        lines.push(format!("  vizier run {flow_label} --set {param}=value"));
    } else {
        lines.push(format!("  vizier run {flow_label} --check"));
    }
    lines.push(String::new());

    lines.push("Run options:".to_string());
    lines.extend(
        [
            "  --set <KEY=VALUE>             Template parameter override (repeatable)",
            "  --check                       Validate queue-time checks without enqueueing",
            "  --after <REF>                 Root dependency: JOB_ID or run:RUN_ID",
            "  --require-approval            Require approval before root jobs start",
            "  --no-require-approval         Disable root approval gating",
            "  --follow                      Wait for terminal run state and stream progress",
            "  --repeat <N>                  Enqueue N serial runs",
            "  --format <text|json>          Output format",
            "  -q/--quiet, -v/--verbose, -d/--debug, --no-ansi, -C/--config-file, -l/--load-session, -n/--no-session",
        ]
        .iter()
        .map(|value| value.to_string()),
    );
    lines.push(String::new());

    Ok(lines.join("\n"))
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
    matches!(arg, "-C" | "--config-file" | "-l" | "--load-session")
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

fn ordered_cli_params(input_spec: &workflow_templates::WorkflowTemplateInputSpec) -> Vec<String> {
    let mut ordered = Vec::<String>::new();
    for param in &input_spec.positional {
        if !ordered.contains(param) {
            ordered.push(param.clone());
        }
    }
    for target in input_spec.named.values() {
        if !ordered.contains(target) {
            ordered.push(target.clone());
        }
    }
    for param in &input_spec.params {
        if !ordered.contains(param) {
            ordered.push(param.clone());
        }
    }
    ordered
}

fn named_entries(
    input_spec: &workflow_templates::WorkflowTemplateInputSpec,
    ordered_params: &[String],
) -> Vec<(String, String)> {
    let mut entries = Vec::<(String, String)>::new();
    let mut aliases = input_spec
        .named
        .iter()
        .map(|(alias, target)| (alias.clone(), target.clone()))
        .collect::<Vec<_>>();
    aliases.sort_by(|(alias_a, target_a), (alias_b, target_b)| {
        let index_a = ordered_params
            .iter()
            .position(|param| param == target_a)
            .unwrap_or(usize::MAX);
        let index_b = ordered_params
            .iter()
            .position(|param| param == target_b)
            .unwrap_or(usize::MAX);
        (index_a, alias_a).cmp(&(index_b, alias_b))
    });
    entries.extend(aliases);

    for param in ordered_params {
        if entries.iter().any(|(_, target)| target == param) {
            continue;
        }
        entries.push((param.clone(), param.clone()));
    }

    entries
}

fn named_example(
    flow_label: &str,
    input_spec: &workflow_templates::WorkflowTemplateInputSpec,
    ordered_params: &[String],
) -> String {
    let mut parts = vec![format!("vizier run {flow_label}")];
    let take = ordered_params.len().clamp(1, 2);
    for param in ordered_params.iter().take(take) {
        let label = cli_label_for_param(input_spec, param);
        let sample = example_value(input_spec, param);
        parts.push(format!("--{} {}", kebab_case_key(&label), sample));
    }
    parts.join(" ")
}

fn positional_example(
    flow_label: &str,
    input_spec: &workflow_templates::WorkflowTemplateInputSpec,
) -> Option<String> {
    if input_spec.positional.is_empty() {
        return None;
    }
    let values = input_spec
        .positional
        .iter()
        .take(2)
        .map(|param| example_value(input_spec, param))
        .collect::<Vec<_>>();
    if values.is_empty() {
        None
    } else {
        Some(format!("vizier run {flow_label} {}", values.join(" ")))
    }
}

fn preferred_cli_alias_for_param<'a>(
    input_spec: &'a workflow_templates::WorkflowTemplateInputSpec,
    param: &str,
) -> Option<&'a str> {
    input_spec.named.iter().find_map(|(alias, target)| {
        if target == param {
            Some(alias.as_str())
        } else {
            None
        }
    })
}

fn cli_label_for_param(
    input_spec: &workflow_templates::WorkflowTemplateInputSpec,
    param: &str,
) -> String {
    preferred_cli_alias_for_param(input_spec, param)
        .unwrap_or(param)
        .to_string()
}

fn kebab_case_key(value: &str) -> String {
    value.trim().replace('_', "-")
}

fn example_value(
    input_spec: &workflow_templates::WorkflowTemplateInputSpec,
    param: &str,
) -> String {
    let label = cli_label_for_param(input_spec, param).to_ascii_lowercase();
    if label.contains("file") || label.contains("path") {
        "LIBRARY.md".to_string()
    } else if label.contains("name") || label.contains("slug") {
        "my-change".to_string()
    } else if label.contains("target") {
        "main".to_string()
    } else if label.contains("branch") {
        "draft/my-change".to_string()
    } else {
        format!("example-{}", kebab_case_key(&label))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{named_example, ordered_cli_params, positional_example, subcommand_from_raw_args};
    use crate::workflow_templates::WorkflowTemplateInputSpec;

    #[test]
    fn subcommand_from_raw_args_skips_global_flag_with_equals() {
        let raw_args = vec![
            "vizier".to_string(),
            "--config-file=./vizier.toml".to_string(),
            "list".to_string(),
        ];

        assert_eq!(
            subcommand_from_raw_args(&raw_args),
            Some("list".to_string())
        );
    }

    #[test]
    fn run_help_examples_use_cli_aliases() {
        let mut named = BTreeMap::new();
        named.insert("file".to_string(), "spec_file".to_string());
        named.insert("name".to_string(), "slug".to_string());
        let input_spec = WorkflowTemplateInputSpec {
            params: vec![
                "branch".to_string(),
                "slug".to_string(),
                "spec_file".to_string(),
            ],
            positional: vec![
                "spec_file".to_string(),
                "slug".to_string(),
                "branch".to_string(),
            ],
            named,
        };

        let ordered = ordered_cli_params(&input_spec);
        assert_eq!(
            named_example("draft", &input_spec, &ordered),
            "vizier run draft --file LIBRARY.md --name my-change"
        );
        assert_eq!(
            positional_example("draft", &input_spec).as_deref(),
            Some("vizier run draft LIBRARY.md my-change")
        );
    }
}
