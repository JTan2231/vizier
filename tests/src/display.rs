use crate::fixtures::*;

#[test]
fn test_no_ansi_suppresses_escape_sequences() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["--no-ansi", "save", "ansi suppression check"])?;
    assert!(
        output.status.success(),
        "vizier save failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains('\u{1b}'),
        "output should not include ANSI escapes when --no-ansi is set: {combined}"
    );
    Ok(())
}
