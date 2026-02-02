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
