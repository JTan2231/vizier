use git2::{Error, ErrorCode, ObjectType, Oid, Repository, Signature, Sort};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReleaseVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl ReleaseVersion {
    pub const ZERO: Self = Self {
        major: 0,
        minor: 0,
        patch: 0,
    };

    pub fn bump(self, bump: ReleaseBump) -> Self {
        match bump {
            ReleaseBump::None => self,
            ReleaseBump::Patch => Self {
                major: self.major,
                minor: self.minor,
                patch: self.patch + 1,
            },
            ReleaseBump::Minor => Self {
                major: self.major,
                minor: self.minor + 1,
                patch: 0,
            },
            ReleaseBump::Major => Self {
                major: self.major + 1,
                minor: 0,
                patch: 0,
            },
        }
    }
}

impl std::fmt::Display for ReleaseVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReleaseBump {
    None,
    Patch,
    Minor,
    Major,
}

impl std::fmt::Display for ReleaseBump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            ReleaseBump::None => "none",
            ReleaseBump::Patch => "patch",
            ReleaseBump::Minor => "minor",
            ReleaseBump::Major => "major",
        };
        write!(f, "{label}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseTag {
    pub name: String,
    pub version: ReleaseVersion,
    pub commit: Oid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseCommit {
    pub oid: Oid,
    pub short_sha: String,
    pub subject: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseSectionKind {
    BreakingChanges,
    Features,
    FixesPerformance,
    Other,
}

impl ReleaseSectionKind {
    pub fn title(self) -> &'static str {
        match self {
            ReleaseSectionKind::BreakingChanges => "Breaking Changes",
            ReleaseSectionKind::Features => "Features",
            ReleaseSectionKind::FixesPerformance => "Fixes/Performance",
            ReleaseSectionKind::Other => "Other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNoteEntry {
    pub short_sha: String,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSection {
    pub kind: ReleaseSectionKind,
    pub entries: Vec<ReleaseNoteEntry>,
    pub overflow: usize,
}

impl ReleaseSection {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.overflow == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub breaking: ReleaseSection,
    pub features: ReleaseSection,
    pub fixes_performance: ReleaseSection,
    pub other: ReleaseSection,
}

impl ReleaseNotes {
    pub fn sections(&self, include_other: bool) -> Vec<&ReleaseSection> {
        let mut out = vec![&self.breaking, &self.features, &self.fixes_performance];
        if include_other {
            out.push(&self.other);
        }
        out
    }

    pub fn render_markdown(&self, include_other: bool) -> String {
        let mut lines = Vec::new();

        for section in self.sections(include_other) {
            if section.is_empty() {
                continue;
            }
            lines.push(format!("### {}", section.kind.title()));
            for entry in &section.entries {
                lines.push(format!("- {} ({})", entry.subject, entry.short_sha));
            }
            if section.overflow > 0 {
                lines.push(format!("- +{} more", section.overflow));
            }
            lines.push(String::new());
        }

        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }

        lines.join("\n")
    }
}

pub fn parse_release_version_tag(tag_name: &str) -> Result<ReleaseVersion, String> {
    let raw = tag_name.trim();
    let version = raw
        .strip_prefix('v')
        .ok_or_else(|| "release tags must start with `v`".to_string())?;
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return Err("release tags must be `v<major>.<minor>.<patch>`".to_string());
    }

    let major = parts[0]
        .parse::<u64>()
        .map_err(|_| format!("invalid major version component `{}`", parts[0]))?;
    let minor = parts[1]
        .parse::<u64>()
        .map_err(|_| format!("invalid minor version component `{}`", parts[1]))?;
    let patch = parts[2]
        .parse::<u64>()
        .map_err(|_| format!("invalid patch version component `{}`", parts[2]))?;

    Ok(ReleaseVersion {
        major,
        minor,
        patch,
    })
}

fn short_oid(oid: Oid) -> String {
    let text = oid.to_string();
    text.chars().take(7).collect()
}

pub fn release_tag_exists(tag_name: &str) -> Result<bool, Error> {
    release_tag_exists_in(".", tag_name)
}

fn release_tag_exists_in<P: AsRef<Path>>(repo_path: P, tag_name: &str) -> Result<bool, Error> {
    let repo = Repository::discover(repo_path)?;
    let reference = format!("refs/tags/{tag_name}");
    match repo.find_reference(&reference) {
        Ok(_) => Ok(true),
        Err(err) if err.code() == ErrorCode::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn latest_reachable_release_tag() -> Result<Option<ReleaseTag>, Error> {
    latest_reachable_release_tag_in(".")
}

fn latest_reachable_release_tag_in<P: AsRef<Path>>(
    repo_path: P,
) -> Result<Option<ReleaseTag>, Error> {
    let repo = Repository::discover(repo_path)?;
    let head = repo.head()?;
    let head_commit = head.peel_to_commit()?;
    let head_oid = head_commit.id();

    let refs = match repo.references_glob("refs/tags/v*") {
        Ok(iter) => iter,
        Err(err) if err.code() == ErrorCode::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    let mut best: Option<ReleaseTag> = None;

    for reference in refs {
        let reference = reference?;
        let Some(name) = reference.shorthand() else {
            continue;
        };

        let peeled = reference.peel(ObjectType::Commit)?;
        let commit = peeled
            .into_commit()
            .map_err(|_| Error::from_str("release tag does not resolve to a commit"))?;

        let reachable =
            commit.id() == head_oid || repo.graph_descendant_of(head_oid, commit.id())?;
        if !reachable {
            continue;
        }

        let version = parse_release_version_tag(name).map_err(|reason| {
            Error::from_str(&format!(
                "invalid release tag `{name}`: {reason}; expected v<major>.<minor>.<patch>"
            ))
        })?;

        let candidate = ReleaseTag {
            name: name.to_string(),
            version,
            commit: commit.id(),
        };

        match &best {
            Some(current) if current.version >= candidate.version => {}
            _ => best = Some(candidate),
        }
    }

    Ok(best)
}

pub fn commits_since_release_tag(
    last_tag: Option<&ReleaseTag>,
) -> Result<Vec<ReleaseCommit>, Error> {
    commits_since_release_tag_in(".", last_tag)
}

fn commits_since_release_tag_in<P: AsRef<Path>>(
    repo_path: P,
    last_tag: Option<&ReleaseTag>,
) -> Result<Vec<ReleaseCommit>, Error> {
    let repo = Repository::discover(repo_path)?;
    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

    let head_oid = repo
        .head()?
        .target()
        .ok_or_else(|| Error::from_str("HEAD does not point to a commit"))?;
    revwalk.push(head_oid)?;

    if let Some(tag) = last_tag {
        revwalk.hide(tag.commit)?;
    }

    let mut commits = Vec::new();

    for oid in revwalk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let subject = commit.summary().unwrap_or("<no subject>").trim();
        let message = commit.message().unwrap_or_default();

        commits.push(ReleaseCommit {
            oid,
            short_sha: short_oid(oid),
            subject: if subject.is_empty() {
                "<no subject>".to_string()
            } else {
                subject.to_string()
            },
            message: message.to_string(),
        });
    }

    Ok(commits)
}

pub fn create_annotated_release_tag(tag_name: &str, annotation: &str) -> Result<Oid, Error> {
    create_annotated_release_tag_in(".", tag_name, annotation)
}

fn create_annotated_release_tag_in<P: AsRef<Path>>(
    repo_path: P,
    tag_name: &str,
    annotation: &str,
) -> Result<Oid, Error> {
    let repo = Repository::discover(repo_path)?;
    let target = repo.head()?.peel(ObjectType::Commit)?;
    let signature = repo
        .signature()
        .or_else(|_| Signature::now("Vizier", "vizier@local"))?;
    repo.tag(tag_name, &target, &signature, annotation, false)
}

fn commit_header(subject: &str) -> &str {
    subject
        .split_once(':')
        .map(|(header, _)| header.trim())
        .unwrap_or_else(|| subject.trim())
}

fn conventional_type_from_header(header: &str) -> Option<String> {
    if header.is_empty() {
        return None;
    }

    let without_scope = header
        .split_once('(')
        .map(|(prefix, _)| prefix)
        .unwrap_or(header)
        .trim();
    let commit_type = without_scope.trim_end_matches('!').trim();
    if commit_type.is_empty() {
        None
    } else {
        Some(commit_type.to_ascii_lowercase())
    }
}

fn commit_is_breaking(subject: &str, message: &str) -> bool {
    let header = commit_header(subject);
    if header.ends_with('!') {
        return true;
    }

    let upper = message.to_ascii_uppercase();
    upper.contains("BREAKING CHANGE") || upper.contains("BREAKING-CHANGE")
}

pub fn classify_commit(subject: &str, message: &str) -> (ReleaseBump, ReleaseSectionKind) {
    if commit_is_breaking(subject, message) {
        return (ReleaseBump::Major, ReleaseSectionKind::BreakingChanges);
    }

    let header = commit_header(subject);
    let commit_type = conventional_type_from_header(header);

    match commit_type.as_deref() {
        Some("feat") => (ReleaseBump::Minor, ReleaseSectionKind::Features),
        Some("fix") | Some("perf") => (ReleaseBump::Patch, ReleaseSectionKind::FixesPerformance),
        _ => (ReleaseBump::None, ReleaseSectionKind::Other),
    }
}

pub fn derive_release_bump(commits: &[ReleaseCommit]) -> ReleaseBump {
    commits.iter().fold(ReleaseBump::None, |current, commit| {
        let (candidate, _) = classify_commit(&commit.subject, &commit.message);
        current.max(candidate)
    })
}

fn push_note(section: &mut ReleaseSection, entry: ReleaseNoteEntry, max_commits: usize) {
    if section.entries.len() < max_commits {
        section.entries.push(entry);
    } else {
        section.overflow += 1;
    }
}

pub fn build_release_notes(commits: &[ReleaseCommit], max_commits: usize) -> ReleaseNotes {
    let cap = max_commits.max(1);
    let mut notes = ReleaseNotes {
        breaking: ReleaseSection {
            kind: ReleaseSectionKind::BreakingChanges,
            entries: Vec::new(),
            overflow: 0,
        },
        features: ReleaseSection {
            kind: ReleaseSectionKind::Features,
            entries: Vec::new(),
            overflow: 0,
        },
        fixes_performance: ReleaseSection {
            kind: ReleaseSectionKind::FixesPerformance,
            entries: Vec::new(),
            overflow: 0,
        },
        other: ReleaseSection {
            kind: ReleaseSectionKind::Other,
            entries: Vec::new(),
            overflow: 0,
        },
    };

    for commit in commits {
        let (_, section_kind) = classify_commit(&commit.subject, &commit.message);
        let entry = ReleaseNoteEntry {
            short_sha: commit.short_sha.clone(),
            subject: commit.subject.clone(),
        };

        match section_kind {
            ReleaseSectionKind::BreakingChanges => push_note(&mut notes.breaking, entry, cap),
            ReleaseSectionKind::Features => push_note(&mut notes.features, entry, cap),
            ReleaseSectionKind::FixesPerformance => {
                push_note(&mut notes.fixes_performance, entry, cap)
            }
            ReleaseSectionKind::Other => push_note(&mut notes.other, entry, cap),
        }
    }

    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{IndexAddOption, Repository, Signature};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    struct TestRepo {
        dir: TempDir,
        repo: Repository,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = tempfile::TempDir::new().expect("tempdir");
            let repo = Repository::init(dir.path()).expect("init repo");
            let mut cfg = repo.config().expect("config");
            cfg.set_str("user.name", "Vizier Test").expect("set name");
            cfg.set_str("user.email", "vizier@test.local")
                .expect("set email");
            Self { dir, repo }
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn write_file(&self, rel: &str, contents: &str) {
            let path = self.path().join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(path, contents).expect("write file");
        }

        fn commit_all(&self, message: &str) -> Oid {
            let mut index = self.repo.index().expect("index");
            index
                .add_all(["."], IndexAddOption::DEFAULT, None)
                .expect("add all");
            index.write().expect("write index");
            let tree_id = index.write_tree().expect("write tree");
            let tree = self.repo.find_tree(tree_id).expect("find tree");
            let sig = self
                .repo
                .signature()
                .or_else(|_| Signature::now("Vizier Test", "vizier@test.local"))
                .expect("signature");
            let parent = self
                .repo
                .head()
                .ok()
                .and_then(|head| head.peel_to_commit().ok());
            let parents: Vec<&git2::Commit<'_>> = parent.as_ref().into_iter().collect();
            self.repo
                .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
                .expect("commit")
        }

        fn checkout(&self, branch: &str) {
            self.repo
                .set_head(&format!("refs/heads/{branch}"))
                .expect("set head");
            self.repo.checkout_head(None).expect("checkout head");
        }

        fn create_branch_at_head(&self, branch: &str) {
            let head = self
                .repo
                .head()
                .expect("head")
                .peel_to_commit()
                .expect("commit");
            self.repo.branch(branch, &head, false).expect("branch");
        }

        fn create_annotated_tag_at_head(&self, name: &str) {
            let head = self
                .repo
                .head()
                .expect("head")
                .peel(ObjectType::Commit)
                .expect("head object");
            let sig = self
                .repo
                .signature()
                .or_else(|_| Signature::now("Vizier Test", "vizier@test.local"))
                .expect("signature");
            self.repo
                .tag(name, &head, &sig, &format!("release {name}"), false)
                .expect("tag");
        }
    }

    fn fake_commit(subject: &str, message: &str) -> ReleaseCommit {
        ReleaseCommit {
            oid: Oid::zero(),
            short_sha: "0000000".to_string(),
            subject: subject.to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn parse_release_version_tag_validates_expected_shape() {
        assert_eq!(
            parse_release_version_tag("v1.2.3").expect("parse"),
            ReleaseVersion {
                major: 1,
                minor: 2,
                patch: 3
            }
        );
        assert!(parse_release_version_tag("1.2.3").is_err());
        assert!(parse_release_version_tag("v1.2").is_err());
        assert!(parse_release_version_tag("v1.2.x").is_err());
    }

    #[test]
    fn classify_commit_applies_breaking_feat_fix_perf_and_other() {
        assert_eq!(
            classify_commit("feat!: refactor API", "feat!: refactor API"),
            (ReleaseBump::Major, ReleaseSectionKind::BreakingChanges)
        );
        assert_eq!(
            classify_commit(
                "docs: mention migration",
                "docs: mention migration\n\nBREAKING CHANGE: config format changed"
            ),
            (ReleaseBump::Major, ReleaseSectionKind::BreakingChanges)
        );
        assert_eq!(
            classify_commit("feat(core): add release", "feat(core): add release"),
            (ReleaseBump::Minor, ReleaseSectionKind::Features)
        );
        assert_eq!(
            classify_commit("fix: patch bug", "fix: patch bug"),
            (ReleaseBump::Patch, ReleaseSectionKind::FixesPerformance)
        );
        assert_eq!(
            classify_commit("perf: tighten loop", "perf: tighten loop"),
            (ReleaseBump::Patch, ReleaseSectionKind::FixesPerformance)
        );
        assert_eq!(
            classify_commit("docs: update guide", "docs: update guide"),
            (ReleaseBump::None, ReleaseSectionKind::Other)
        );
    }

    #[test]
    fn derive_release_bump_uses_highest_precedence() {
        let commits = vec![
            fake_commit("docs: update", "docs: update"),
            fake_commit("fix: patch", "fix: patch"),
            fake_commit("feat: add", "feat: add"),
            fake_commit("refactor!: break API", "refactor!: break API"),
        ];

        assert_eq!(derive_release_bump(&commits), ReleaseBump::Major);
    }

    #[test]
    fn build_release_notes_caps_sections_and_records_overflow() {
        let commits = vec![
            fake_commit("feat: one", "feat: one"),
            fake_commit("feat: two", "feat: two"),
            fake_commit("feat: three", "feat: three"),
            fake_commit("docs: note", "docs: note"),
        ];

        let notes = build_release_notes(&commits, 2);
        assert_eq!(notes.features.entries.len(), 2);
        assert_eq!(notes.features.overflow, 1);
        assert_eq!(notes.other.entries.len(), 1);

        let rendered = notes.render_markdown(true);
        assert!(rendered.contains("### Features"));
        assert!(rendered.contains("- +1 more"));
        assert!(rendered.contains("### Other"));
    }

    #[test]
    fn latest_reachable_release_tag_uses_highest_reachable_semver() {
        let repo = TestRepo::new();

        repo.write_file("file.txt", "base\n");
        repo.commit_all("feat: base");
        repo.create_annotated_tag_at_head("v0.1.0");

        repo.create_branch_at_head("side");
        repo.checkout("side");
        repo.write_file("file.txt", "side\n");
        repo.commit_all("feat: side");
        repo.create_annotated_tag_at_head("v9.0.0");

        repo.checkout("master");
        repo.write_file("file.txt", "main\n");
        repo.commit_all("feat: main");
        repo.create_annotated_tag_at_head("v0.2.0");

        let tag = latest_reachable_release_tag_in(repo.path())
            .expect("lookup succeeds")
            .expect("tag exists");
        assert_eq!(tag.name, "v0.2.0");
        assert_eq!(
            tag.version,
            ReleaseVersion {
                major: 0,
                minor: 2,
                patch: 0
            }
        );
    }

    #[test]
    fn latest_reachable_release_tag_errors_on_invalid_reachable_v_tag() {
        let repo = TestRepo::new();

        repo.write_file("file.txt", "base\n");
        repo.commit_all("feat: base");
        repo.create_annotated_tag_at_head("v1.x.0");

        let err =
            latest_reachable_release_tag_in(repo.path()).expect_err("invalid tag should fail");
        assert!(
            err.message().contains("invalid release tag `v1.x.0`"),
            "unexpected error: {}",
            err.message()
        );
    }

    #[test]
    fn commits_since_release_tag_excludes_tagged_commit() {
        let repo = TestRepo::new();

        repo.write_file("notes.txt", "one\n");
        repo.commit_all("feat: one");
        repo.create_annotated_tag_at_head("v0.1.0");

        repo.write_file("notes.txt", "two\n");
        let second = repo.commit_all("fix: two");
        repo.write_file("notes.txt", "three\n");
        let third = repo.commit_all("feat: three");

        let tag = latest_reachable_release_tag_in(repo.path())
            .expect("lookup")
            .expect("tag expected");
        let commits = commits_since_release_tag_in(repo.path(), Some(&tag)).expect("commits");

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].oid, third);
        assert_eq!(commits[1].oid, second);
    }
}
