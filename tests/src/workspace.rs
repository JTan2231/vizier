use crate::fixtures::*;

#[test]
fn test_cd_is_deprecated() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["cd", "workspace-check"])?;
    assert!(
        !output.status.success(),
        "vizier cd should fail when deprecated"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("vizier cd is deprecated"),
        "expected deprecation message in stderr:\n{stderr}"
    );
    Ok(())
}

#[test]
fn test_clean_is_deprecated() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let output = repo.vizier_output(&["clean", "workspace-check", "--yes"])?;
    assert!(
        !output.status.success(),
        "vizier clean should fail when deprecated"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("vizier clean is deprecated"),
        "expected deprecation message in stderr:\n{stderr}"
    );
    Ok(())
}
