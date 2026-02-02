use std::io::{self, IsTerminal, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewQueueChoice {
    ApplyFixes,
    CritiqueOnly,
    ReviewFile,
    Cancel,
}

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

pub(crate) fn prompt_review_queue_choice() -> Result<ReviewQueueChoice, Box<dyn std::error::Error>>
{
    if !io::stdin().is_terminal() {
        return Err(
            "confirmation requires a TTY; rerun with --yes, --review-only, or --review-file".into(),
        );
    }
    eprint!(
        "Review mode?\n  1) Apply fixes automatically\n  2) Critique only\n  3) Write critique to file\n  q) Cancel\n> "
    );
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(parse_review_queue_choice(&answer))
}

fn parse_yes_no(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn parse_review_queue_choice(answer: &str) -> ReviewQueueChoice {
    match answer.trim().to_ascii_lowercase().as_str() {
        "1" | "apply" | "apply fixes" | "fixes" | "yes" => ReviewQueueChoice::ApplyFixes,
        "2" | "critique" | "review-only" | "review only" => ReviewQueueChoice::CritiqueOnly,
        "3" | "file" | "review-file" | "review file" => ReviewQueueChoice::ReviewFile,
        "q" | "quit" | "cancel" | "" => ReviewQueueChoice::Cancel,
        _ => ReviewQueueChoice::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::{ReviewQueueChoice, parse_review_queue_choice, parse_yes_no};

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

    #[test]
    fn parse_review_queue_choice_maps_inputs() {
        assert_eq!(
            parse_review_queue_choice("1"),
            ReviewQueueChoice::ApplyFixes
        );
        assert_eq!(
            parse_review_queue_choice("2"),
            ReviewQueueChoice::CritiqueOnly
        );
        assert_eq!(
            parse_review_queue_choice("3"),
            ReviewQueueChoice::ReviewFile
        );
        assert_eq!(parse_review_queue_choice("q"), ReviewQueueChoice::Cancel);
        assert_eq!(parse_review_queue_choice(""), ReviewQueueChoice::Cancel);
        assert_eq!(
            parse_review_queue_choice("unknown"),
            ReviewQueueChoice::Cancel
        );
    }
}
