pub(crate) fn flag_present(args: &[String], short: Option<char>, long: &str) -> bool {
    let short_flag = short.map(|value| format!("-{value}"));
    args.iter().any(|arg| {
        if arg == long || (long.starts_with("--") && arg.starts_with(&format!("{long}="))) {
            return true;
        }
        if let Some(short_flag) = short_flag.as_ref() {
            if arg == short_flag {
                return true;
            }
            if arg.starts_with('-')
                && !arg.starts_with("--")
                && arg.contains(short_flag.trim_start_matches('-'))
            {
                return true;
            }
        }
        false
    })
}

pub(crate) fn normalize_run_invocation_args(args: &[String]) -> Vec<String> {
    let Some(run_index) = find_run_subcommand_index(args) else {
        return args.to_vec();
    };
    if run_index + 1 >= args.len() {
        return args.to_vec();
    }

    let mut normalized = Vec::with_capacity(args.len() + 8);
    normalized.extend_from_slice(&args[..=run_index + 1]);

    let mut index = run_index + 2;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            normalized.extend_from_slice(&args[index..]);
            break;
        }

        if is_option_with_value(token, "--set")
            || is_option_with_value(token, "--after")
            || is_option_with_value(token, "--format")
            || is_option_with_value(token, "--load-session")
            || is_option_with_value(token, "--config-file")
            || is_short_option_with_value(token, 'l')
            || is_short_option_with_value(token, 'C')
        {
            normalized.push(token.clone());
            if option_token_requires_next_value(token) && index + 1 < args.len() {
                index += 1;
                normalized.push(args[index].clone());
            }
            index += 1;
            continue;
        }

        if is_flag_option(token, "--require-approval")
            || is_flag_option(token, "--no-require-approval")
            || is_flag_option(token, "--follow")
            || is_flag_option(token, "--verbose")
            || is_flag_option(token, "--quiet")
            || is_flag_option(token, "--debug")
            || is_flag_option(token, "--no-ansi")
            || is_flag_option(token, "--no-pager")
            || is_flag_option(token, "--no-session")
            || is_flag_option(token, "--help")
            || is_flag_option(token, "--version")
            || is_short_flag_token(token)
        {
            normalized.push(token.clone());
            index += 1;
            continue;
        }

        if token.starts_with("--") {
            let (key, value, consumed_next) = parse_dynamic_flag_value(args, index);
            if key.is_empty() {
                normalized.push(token.clone());
                if consumed_next {
                    index += 1;
                    normalized.push(args[index].clone());
                }
            } else {
                normalized.push("--set".to_string());
                normalized.push(format!("{key}={value}"));
                if consumed_next {
                    index += 1;
                }
            }
            index += 1;
            continue;
        }

        normalized.push(token.clone());
        index += 1;
    }

    normalized
}

fn find_run_subcommand_index(args: &[String]) -> Option<usize> {
    let mut index = 1;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            return None;
        }
        if token == "run" {
            return Some(index);
        }

        if is_option_with_value(token, "--load-session")
            || is_option_with_value(token, "--config-file")
            || is_short_option_with_value(token, 'l')
            || is_short_option_with_value(token, 'C')
        {
            if option_token_requires_next_value(token) {
                index += 1;
            }
            index += 1;
            continue;
        }

        if token.starts_with('-') {
            index += 1;
            continue;
        }

        return None;
    }
    None
}

fn parse_dynamic_flag_value(args: &[String], flag_index: usize) -> (String, String, bool) {
    let flag = &args[flag_index];
    let raw = flag.trim_start_matches("--");
    if let Some((raw_key, raw_value)) = raw.split_once('=') {
        return (
            canonicalize_workflow_param_key(raw_key),
            raw_value.to_string(),
            false,
        );
    }

    if let Some(next) = args.get(flag_index + 1)
        && !looks_like_option(next)
    {
        return (canonicalize_workflow_param_key(raw), next.clone(), true);
    }

    (
        canonicalize_workflow_param_key(raw),
        "true".to_string(),
        false,
    )
}

fn canonicalize_workflow_param_key(raw: &str) -> String {
    raw.trim().replace('-', "_")
}

fn is_option_with_value(token: &str, long: &str) -> bool {
    token == long || token.starts_with(&format!("{long}="))
}

fn is_flag_option(token: &str, long: &str) -> bool {
    token == long
}

fn is_short_option_with_value(token: &str, short: char) -> bool {
    token == format!("-{short}") || (token.starts_with(&format!("-{short}")) && token.len() > 2)
}

fn option_token_requires_next_value(token: &str) -> bool {
    !(token.starts_with("--") && token.contains('='))
        && !((token.starts_with("-l") || token.starts_with("-C")) && token.len() > 2)
}

fn is_short_flag_token(token: &str) -> bool {
    if !token.starts_with('-') || token.starts_with("--") {
        return false;
    }
    token
        .chars()
        .skip(1)
        .all(|ch| matches!(ch, 'v' | 'q' | 'd' | 'n' | 'h' | 'V'))
}

fn looks_like_option(value: &str) -> bool {
    value.len() > 1 && value.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::normalize_run_invocation_args;

    #[test]
    fn normalize_run_rewrites_unknown_long_flags_to_set() {
        let args = vec![
            "vizier".to_string(),
            "run".to_string(),
            "draft".to_string(),
            "--spec-file".to_string(),
            "specs/DEFAULT.md".to_string(),
            "--follow".to_string(),
        ];

        let normalized = normalize_run_invocation_args(&args);
        assert_eq!(
            normalized,
            vec![
                "vizier",
                "run",
                "draft",
                "--set",
                "spec_file=specs/DEFAULT.md",
                "--follow"
            ]
        );
    }

    #[test]
    fn normalize_run_rewrites_unknown_flag_with_equals() {
        let args = vec![
            "vizier".to_string(),
            "run".to_string(),
            "draft".to_string(),
            "--slug=my-change".to_string(),
        ];

        let normalized = normalize_run_invocation_args(&args);
        assert_eq!(
            normalized,
            vec!["vizier", "run", "draft", "--set", "slug=my-change"]
        );
    }

    #[test]
    fn normalize_run_rewrites_unknown_boolean_flag_to_true() {
        let args = vec![
            "vizier".to_string(),
            "run".to_string(),
            "draft".to_string(),
            "--dry".to_string(),
        ];

        let normalized = normalize_run_invocation_args(&args);
        assert_eq!(
            normalized,
            vec!["vizier", "run", "draft", "--set", "dry=true"]
        );
    }

    #[test]
    fn normalize_run_leaves_non_run_commands_unchanged() {
        let args = vec![
            "vizier".to_string(),
            "jobs".to_string(),
            "list".to_string(),
            "--all".to_string(),
        ];

        assert_eq!(normalize_run_invocation_args(&args), args);
    }

    #[test]
    fn normalize_run_preserves_known_run_options() {
        let args = vec![
            "vizier".to_string(),
            "--quiet".to_string(),
            "run".to_string(),
            "draft".to_string(),
            "--after".to_string(),
            "job-123".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--follow".to_string(),
        ];

        assert_eq!(normalize_run_invocation_args(&args), args);
    }
}
