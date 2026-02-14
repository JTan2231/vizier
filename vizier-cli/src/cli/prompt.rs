use std::io::{self, IsTerminal, Write};

pub(crate) fn prompt_yes_no(prompt: &str) -> Result<bool, Box<dyn std::error::Error>> {
    if !io::stdin().is_terminal() {
        return Err("confirmation requires a TTY; rerun with --yes".into());
    }
    eprint!("{prompt} [y/N]: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(parse_yes_no(&answer))
}

fn parse_yes_no(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::parse_yes_no;

    #[test]
    fn parse_yes_no_accepts_yes_variants() {
        assert!(parse_yes_no("y"));
        assert!(parse_yes_no("Y"));
        assert!(parse_yes_no("yes"));
        assert!(parse_yes_no(" Yes "));
        assert!(!parse_yes_no("n"));
        assert!(!parse_yes_no("no"));
        assert!(!parse_yes_no(""));
    }
}
