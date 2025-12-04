use git2::{
    BranchType, ErrorCode, Repository, StatusOptions, WorktreeAddOptions, WorktreePruneOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const WORKSPACE_DIR_PREFIX: &str = "workspace-";
const WORKTREE_NAME_PREFIX: &str = "vizier-workspace-";
const MANIFEST_FILE: &str = "workspaces.json";

#[derive(Debug, Clone)]
pub struct WorkspaceStatus {
    pub branch: String,
    pub path: PathBuf,
    pub worktree_name: String,
    pub created: bool,
    pub clean: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceCandidate {
    pub slug: String,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub worktree_name: String,
    pub registered: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct WorkspaceManifest {
    entries: BTreeMap<String, ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestEntry {
    branch: String,
    worktree_name: String,
    path: String,
}

impl WorkspaceManifest {
    fn from_path(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if path.exists() {
            let contents = fs::read_to_string(path)?;
            let manifest: Self = serde_json::from_str(&contents)?;
            Ok(manifest)
        } else {
            Ok(Self::default())
        }
    }

    fn write(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }
}

pub struct WorkspaceStore {
    repo_root: PathBuf,
    manifest_path: PathBuf,
    manifest: WorkspaceManifest,
}

impl WorkspaceStore {
    pub fn load(repo_root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let tmp_root = repo_root.join(".vizier/tmp-worktrees");
        fs::create_dir_all(&tmp_root)?;
        let manifest_path = tmp_root.join(MANIFEST_FILE);
        let manifest = WorkspaceManifest::from_path(&manifest_path)?;

        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            manifest_path,
            manifest,
        })
    }

    pub fn ensure_workspace(
        &mut self,
        slug: &str,
        branch: &str,
    ) -> Result<WorkspaceStatus, Box<dyn std::error::Error>> {
        let repo = Repository::open(&self.repo_root)?;
        let expected_path = workspace_path(&self.repo_root, slug);
        let expected_worktree = workspace_worktree_name(slug);
        let mut manifest_changed = false;

        if let Some(entry) = self.manifest.entries.get(slug) {
            if let Some(status) = self.validate_entry(&repo, slug, branch, entry)? {
                if entry.branch != branch {
                    manifest_changed = true;
                }
                if manifest_changed {
                    self.persist_entry(slug, &status)?;
                }
                return Ok(status);
            }

            // Drop stale manifest entries so follow-on creation succeeds.
            self.manifest.entries.remove(slug);
            manifest_changed = true;
        }

        if let Some(status) =
            self.validate_worktree(&repo, slug, branch, &expected_worktree, &expected_path)?
        {
            self.persist_entry(slug, &status)?;
            return Ok(status);
        }

        if !branch_exists(&repo, branch)? {
            return Err(
                format!(
                    "branch {} does not exist; create it (for example with `vizier draft {slug}`) before running vizier cd",
                    branch
                )
                .into(),
            );
        }

        if expected_path.exists() && expected_path.read_dir()?.next().is_some() {
            return Err(format!(
                "workspace path {} already exists and is not empty; clean it with `vizier clean {slug}` before retrying",
                expected_path.display()
            )
            .into());
        }

        add_worktree(&repo, &expected_worktree, &expected_path, branch)?;

        let status = WorkspaceStatus {
            branch: branch.to_string(),
            path: expected_path.clone(),
            worktree_name: expected_worktree,
            created: true,
            clean: Some(true),
        };

        self.persist_entry(slug, &status)?;
        if manifest_changed {
            self.save()?;
        }

        Ok(status)
    }

    pub fn discover(
        &self,
        slug_filter: Option<&str>,
    ) -> Result<Vec<WorkspaceCandidate>, Box<dyn std::error::Error>> {
        let repo = Repository::open(&self.repo_root)?;
        let mut candidates: BTreeMap<String, WorkspaceCandidate> = BTreeMap::new();

        for (slug, entry) in &self.manifest.entries {
            if let Some(filter) = slug_filter {
                if slug != filter {
                    continue;
                }
            }

            let path = self.repo_root.join(&entry.path);
            candidates.insert(
                slug.clone(),
                WorkspaceCandidate {
                    slug: slug.clone(),
                    branch: Some(entry.branch.clone()),
                    path,
                    worktree_name: entry.worktree_name.clone(),
                    registered: false,
                },
            );
        }

        for worktree_name in repo.worktrees()?.iter().flatten() {
            if !worktree_name.starts_with(WORKTREE_NAME_PREFIX) {
                continue;
            }
            let Some(slug) = worktree_name.strip_prefix(WORKTREE_NAME_PREFIX) else {
                continue;
            };
            let slug = slug.to_string();

            if let Some(filter) = slug_filter {
                if slug != filter {
                    continue;
                }
            }

            if let Ok(worktree) = repo.find_worktree(worktree_name) {
                let path = worktree.path().to_path_buf();
                let branch = head_branch(&path).ok();
                let entry = candidates.entry(slug.clone()).or_insert_with(|| WorkspaceCandidate {
                    slug: slug.clone(),
                    branch: branch.clone(),
                    path: path.clone(),
                    worktree_name: worktree_name.to_string(),
                    registered: true,
                });

                entry.registered = true;
                entry.path = path;
                if entry.branch.is_none() {
                    entry.branch = branch;
                }
                if entry.worktree_name.is_empty() {
                    entry.worktree_name = worktree_name.to_string();
                }
            }
        }

        let tmp_root = self.repo_root.join(".vizier/tmp-worktrees");
        if tmp_root.exists() {
            for entry in fs::read_dir(&tmp_root)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if !name_str.starts_with(WORKSPACE_DIR_PREFIX) {
                    continue;
                }
                let slug = name_str.trim_start_matches(WORKSPACE_DIR_PREFIX).to_string();
                if let Some(filter) = slug_filter {
                    if slug != filter {
                        continue;
                    }
                }

                candidates.entry(slug.clone()).or_insert_with(|| WorkspaceCandidate {
                    slug: slug.clone(),
                    branch: None,
                    path: entry.path(),
                    worktree_name: workspace_worktree_name(&slug),
                    registered: false,
                });
            }
        }

        Ok(candidates.into_values().collect())
    }

    pub fn forget(&mut self, slug: &str) -> bool {
        self.manifest.entries.remove(slug).is_some()
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.manifest.write(&self.manifest_path)
    }

    fn validate_entry(
        &self,
        repo: &Repository,
        slug: &str,
        expected_branch: &str,
        entry: &ManifestEntry,
    ) -> Result<Option<WorkspaceStatus>, Box<dyn std::error::Error>> {
        let path = self.repo_root.join(&entry.path);
        if !path.exists() {
            return Ok(None);
        }

        let worktree = match repo.find_worktree(&entry.worktree_name) {
            Ok(tree) => tree,
            Err(_) => return Ok(None),
        };

        if worktree.path() != path {
            return Ok(None);
        }

        let head_branch = head_branch(&path)?;
        if head_branch != expected_branch {
            return Err(format!(
                "workspace {} points to branch {} (expected {}); clean it with `vizier clean {slug}` or check out the correct branch inside the workspace",
                path.display(),
                head_branch,
                expected_branch
            )
            .into());
        }

        let clean = worktree_cleanliness(&path).ok();
        Ok(Some(WorkspaceStatus {
            branch: expected_branch.to_string(),
            path,
            worktree_name: entry.worktree_name.clone(),
            created: false,
            clean,
        }))
    }

    fn validate_worktree(
        &self,
        repo: &Repository,
        slug: &str,
        expected_branch: &str,
        worktree_name: &str,
        path: &Path,
    ) -> Result<Option<WorkspaceStatus>, Box<dyn std::error::Error>> {
        if !path.exists() {
            return Ok(None);
        }

        let worktree = match repo.find_worktree(worktree_name) {
            Ok(tree) => tree,
            Err(_) => return Ok(None),
        };

        if worktree.path() != path {
            return Ok(None);
        }

        let head_branch = head_branch(path)?;
        if head_branch != expected_branch {
            return Err(format!(
                "workspace {} points to branch {} (expected {}); clean it with `vizier clean {slug}` or check out the correct branch inside the workspace",
                path.display(),
                head_branch,
                expected_branch
            )
            .into());
        }

        let clean = worktree_cleanliness(path).ok();
        Ok(Some(WorkspaceStatus {
            branch: expected_branch.to_string(),
            path: path.to_path_buf(),
            worktree_name: worktree_name.to_string(),
            created: false,
            clean,
        }))
    }

    fn persist_entry(
        &mut self,
        slug: &str,
        status: &WorkspaceStatus,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let rel_path = status
            .path
            .strip_prefix(&self.repo_root)
            .unwrap_or(&status.path)
            .to_string_lossy()
            .to_string();

        self.manifest.entries.insert(
            slug.to_string(),
            ManifestEntry {
                branch: status.branch.clone(),
                worktree_name: status.worktree_name.clone(),
                path: rel_path,
            },
        );
        self.save()
    }
}

pub fn workspace_path(repo_root: &Path, slug: &str) -> PathBuf {
    repo_root
        .join(".vizier/tmp-worktrees")
        .join(format!("{WORKSPACE_DIR_PREFIX}{slug}"))
}

pub fn workspace_worktree_name(slug: &str) -> String {
    format!("{WORKTREE_NAME_PREFIX}{slug}")
}

pub fn worktree_cleanliness(path: &Path) -> Result<bool, git2::Error> {
    let repo = Repository::open(path)?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut opts))?;
    Ok(statuses.is_empty())
}

pub fn remove_workspace(
    repo_root: &Path,
    candidate: &WorkspaceCandidate,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo = Repository::open(repo_root)?;
    if candidate.registered {
        remove_worktree(&repo, &candidate.worktree_name, true)?;
    }
    if candidate.path.exists() {
        fs::remove_dir_all(&candidate.path)?;
    }
    Ok(())
}

fn branch_exists(repo: &Repository, branch: &str) -> Result<bool, git2::Error> {
    match repo.find_branch(branch, BranchType::Local) {
        Ok(_) => Ok(true),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn head_branch(path: &Path) -> Result<String, git2::Error> {
    let repo = Repository::open(path)?;
    let head = repo.head()?;
    if !head.is_branch() {
        return Err(git2::Error::from_str("workspace HEAD is detached"));
    }
    head.shorthand()
        .map(|value| value.to_string())
        .ok_or_else(|| git2::Error::from_str("workspace HEAD is missing a branch name"))
}

fn add_worktree(
    repo: &Repository,
    worktree_name: &str,
    path: &Path,
    branch: &str,
) -> Result<(), git2::Error> {
    let mut opts = WorktreeAddOptions::new();
    let reference = repo.find_reference(&format!("refs/heads/{branch}"))?;
    opts.reference(Some(&reference));
    repo.worktree(worktree_name, path, Some(&opts))?;
    Ok(())
}

fn remove_worktree(
    repo: &Repository,
    worktree_name: &str,
    remove_dir: bool,
) -> Result<(), git2::Error> {
    let worktree = repo.find_worktree(worktree_name)?;
    let mut opts = WorktreePruneOptions::new();
    opts.valid(true).locked(true).working_tree(remove_dir);
    worktree.prune(Some(&mut opts))
}
