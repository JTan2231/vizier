use crate::fixtures::*;

#[test]
fn test_refine_questions_only() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let slug = "refine-questions";
    let draft = repo.vizier_output(&["draft", "--name", slug, "refine question spec"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let repo_handle = repo.repo();
    let before_commit = repo_handle
        .find_branch(&format!("draft/{slug}"), BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_IT_SKIP_CODE_CHANGE", "1");
    cmd.env("VIZIER_IT_SKIP_VIZIER_CHANGE", "1");
    cmd.args(["refine", slug]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier refine failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--- Plan refine questions for plan refine-questions ---"),
        "refine questions output missing header:\n{stdout}"
    );

    let after_commit = repo_handle
        .find_branch(&format!("draft/{slug}"), BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();
    assert_eq!(
        after_commit, before_commit,
        "refine questions should not create a new commit"
    );
    Ok(())
}
#[test]
fn test_refine_updates_plan_with_clarifications() -> TestResult {
    let repo = IntegrationRepo::new()?;
    let slug = "refine-update";
    let draft = repo.vizier_output(&["draft", "--name", slug, "refine update spec"])?;
    assert!(
        draft.status.success(),
        "vizier draft failed: {}",
        String::from_utf8_lossy(&draft.stderr)
    );

    let repo_handle = repo.repo();
    let before_commit = repo_handle
        .find_branch(&format!("draft/{slug}"), BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();

    let mut cmd = repo.vizier_cmd();
    cmd.env("VIZIER_IT_SKIP_CODE_CHANGE", "1");
    cmd.env("VIZIER_IT_SKIP_VIZIER_CHANGE", "1");
    cmd.args(["refine", slug, "Clarify rollout order for the pipeline."]);
    let output = cmd.output()?;
    assert!(
        output.status.success(),
        "vizier refine with clarifications failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after_commit = repo_handle
        .find_branch(&format!("draft/{slug}"), BranchType::Local)?
        .get()
        .peel_to_commit()?
        .id();
    assert_ne!(
        after_commit, before_commit,
        "refine updates should add a new commit to the plan branch"
    );

    let files = files_changed_in_commit(&repo_handle, &format!("draft/{slug}"))?;
    assert_eq!(
        files.len(),
        1,
        "refine update should only touch the plan document, got {files:?}"
    );
    assert!(
        files.contains(&format!(".vizier/implementation-plans/{slug}.md")),
        "refine update should only touch the plan document, got {files:?}"
    );

    let branch = repo_handle.find_branch(&format!("draft/{slug}"), BranchType::Local)?;
    let commit = branch.get().peel_to_commit()?;
    let tree = commit.tree()?;
    let entry = tree.get_path(Path::new(&format!(
        ".vizier/implementation-plans/{slug}.md"
    )))?;
    let blob = repo_handle.find_blob(entry.id())?;
    let contents = std::str::from_utf8(blob.content())?;
    assert!(
        contents.contains("## Clarifications"),
        "updated plan should include a clarifications section"
    );
    assert!(
        contents.contains("Clarify rollout order for the pipeline."),
        "updated plan should include the clarification text"
    );
    Ok(())
}
