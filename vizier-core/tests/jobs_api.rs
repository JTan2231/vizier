use tempfile::TempDir;

#[test]
fn core_jobs_api_is_callable_without_cli_modules() {
    let repo = TempDir::new().expect("temp repo");
    let jobs_root = vizier_core::jobs::ensure_jobs_root(repo.path()).expect("jobs root");

    assert!(jobs_root.exists());
    assert!(jobs_root.ends_with(".vizier/jobs"));
    assert_eq!(
        vizier_core::jobs::status_label(vizier_core::jobs::JobStatus::Queued),
        "queued"
    );
}
