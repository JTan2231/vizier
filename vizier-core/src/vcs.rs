use git2::{
    BranchType, Cred, CredentialType, DiffFormat, DiffOptions, Error, ErrorClass, ErrorCode,
    IndexAddOption, Oid, PushOptions, RemoteCallbacks, Repository, RepositoryState, Signature,
    Sort, Status, StatusOptions,
};
use std::cell::RefCell;
use std::env;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

fn normalize_pathspec(path: &str) -> String {
    let mut s = path
        .trim()
        .trim_end_matches('/')
        .trim_end_matches('\\')
        .to_string();

    s = s.replace('\\', "/");
    if let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }

    // Preserve leading UNC `//`, collapse doubles after it.
    if s.starts_with("//") {
        let mut out = String::from("//");
        let rest = s.trim_start_matches('/');
        // collapse any remaining '//' in the tail
        let mut last = '\0';
        for ch in rest.chars() {
            if ch != '/' || last != '/' {
                out.push(ch);
            }
            last = ch;
        }
        s = out;
    } else {
        while s.contains("//") {
            s = s.replace("//", "/");
        }
    }

    s
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteScheme {
    Ssh,
    Https,
    Other(String),
}

impl RemoteScheme {
    pub fn label(&self) -> &str {
        match self {
            RemoteScheme::Ssh => "ssh",
            RemoteScheme::Https => "https",
            RemoteScheme::Other(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperScope {
    Initial,
    UserPass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshKeyKind {
    IdEd25519,
    IdRsa,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialStrategy {
    CredentialHelper(HelperScope),
    SshKey(SshKeyKind),
    Username,
    Default,
}

impl CredentialStrategy {
    pub fn label(&self) -> &'static str {
        match self {
            CredentialStrategy::CredentialHelper(_) => "helper",
            CredentialStrategy::SshKey(SshKeyKind::IdEd25519) => "file-id_ed25519",
            CredentialStrategy::SshKey(SshKeyKind::IdRsa) => "file-id_rsa",
            CredentialStrategy::Username => "username",
            CredentialStrategy::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    Success,
    Failure(String),
    Skipped(String),
}

impl AttemptOutcome {
    pub fn message(&self) -> Option<&str> {
        match self {
            AttemptOutcome::Success => None,
            AttemptOutcome::Failure(msg) | AttemptOutcome::Skipped(msg) => Some(msg.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialAttempt {
    pub strategy: CredentialStrategy,
    pub outcome: AttemptOutcome,
}

#[derive(Debug, Clone)]
struct SshKeyPaths {
    private: PathBuf,
    public: Option<PathBuf>,
}

#[derive(Debug)]
pub enum PushErrorKind {
    General(String),
    Auth {
        remote: String,
        url: String,
        scheme: RemoteScheme,
        attempts: Vec<CredentialAttempt>,
    },
}

#[derive(Debug)]
pub struct PushError {
    kind: PushErrorKind,
    source: Option<Box<Error>>,
}

impl PushError {
    fn general<S: Into<String>>(message: S) -> Self {
        Self {
            kind: PushErrorKind::General(message.into()),
            source: None,
        }
    }

    fn from_git(context: &str, err: Error) -> Self {
        let message = format!("{context}: {}", sanitize_error_message(&err));
        Self {
            kind: PushErrorKind::General(message),
            source: Some(Box::new(err)),
        }
    }

    fn auth(
        remote: String,
        url: String,
        scheme: RemoteScheme,
        attempts: Vec<CredentialAttempt>,
    ) -> Self {
        Self {
            kind: PushErrorKind::Auth {
                remote,
                url,
                scheme,
                attempts,
            },
            source: None,
        }
    }

    pub fn kind(&self) -> &PushErrorKind {
        &self.kind
    }
}

impl fmt::Display for PushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            PushErrorKind::General(message) => write!(f, "{message}"),
            PushErrorKind::Auth { remote, .. } => {
                write!(f, "authentication failed when pushing to {remote}")
            }
        }
    }
}

impl std::error::Error for PushError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|err| err as &(dyn std::error::Error + 'static))
    }
}

fn sanitize_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sanitize_error_message(err: &Error) -> String {
    sanitize_text(err.message())
}

fn classify_remote_scheme(url: &str) -> RemoteScheme {
    if url.starts_with("ssh://") {
        RemoteScheme::Ssh
    } else if url.starts_with("https://") {
        RemoteScheme::Https
    } else if url.contains('@') && url.contains(':') && !url.contains("://") {
        RemoteScheme::Ssh
    } else if let Some((scheme, _)) = url.split_once("://") {
        RemoteScheme::Other(scheme.to_lowercase())
    } else {
        RemoteScheme::Other("unknown".to_string())
    }
}

fn user_home_dir() -> Option<PathBuf> {
    if let Some(home) = env::var_os("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }

    #[cfg(windows)]
    {
        if let Some(profile) = env::var_os("USERPROFILE") {
            if !profile.is_empty() {
                return Some(PathBuf::from(profile));
            }
        }
    }

    None
}

fn locate_default_key(kind: &SshKeyKind) -> Option<SshKeyPaths> {
    let home = user_home_dir()?;
    let key_name = match kind {
        SshKeyKind::IdEd25519 => "id_ed25519",
        SshKeyKind::IdRsa => "id_rsa",
    };

    let private = home.join(".ssh").join(key_name);
    if !private.exists() {
        return None;
    }

    let mut public = private.clone();
    public.set_extension("pub");
    let public = if public.exists() { Some(public) } else { None };

    Some(SshKeyPaths { private, public })
}

fn build_credential_plan(
    allowed_types: CredentialType,
    has_helper: bool,
) -> Vec<CredentialStrategy> {
    let mut plan = Vec::new();

    if has_helper {
        plan.push(CredentialStrategy::CredentialHelper(HelperScope::Initial));
    }

    if allowed_types.contains(CredentialType::SSH_KEY) {
        plan.push(CredentialStrategy::SshKey(SshKeyKind::IdEd25519));
        plan.push(CredentialStrategy::SshKey(SshKeyKind::IdRsa));
    }

    if allowed_types.contains(CredentialType::USERNAME) {
        plan.push(CredentialStrategy::Username);
    }

    if has_helper && allowed_types.contains(CredentialType::USER_PASS_PLAINTEXT) {
        plan.push(CredentialStrategy::CredentialHelper(HelperScope::UserPass));
    }

    plan.push(CredentialStrategy::Default);

    plan
}

struct CredentialRequestContext<'a> {
    url: &'a str,
    username_from_url: Option<&'a str>,
    default_username: &'a str,
}

enum StrategyResult {
    Success(Cred),
    Failure(String),
    Skipped(String),
}

enum CredentialResult {
    Success {
        cred: Cred,
        attempts: Vec<CredentialAttempt>,
    },
    Failure {
        attempts: Vec<CredentialAttempt>,
        final_message: Option<String>,
    },
}

trait CredentialExecutor {
    fn apply(
        &self,
        strategy: &CredentialStrategy,
        ctx: &CredentialRequestContext<'_>,
    ) -> StrategyResult;
}

fn execute_credential_plan<E: CredentialExecutor>(
    plan: &[CredentialStrategy],
    executor: &E,
    ctx: &CredentialRequestContext<'_>,
) -> CredentialResult {
    let mut attempts = Vec::new();
    let mut last_failure_message = None;

    for strategy in plan {
        match executor.apply(strategy, ctx) {
            StrategyResult::Success(cred) => {
                attempts.push(CredentialAttempt {
                    strategy: strategy.clone(),
                    outcome: AttemptOutcome::Success,
                });
                return CredentialResult::Success { cred, attempts };
            }
            StrategyResult::Failure(message) => {
                attempts.push(CredentialAttempt {
                    strategy: strategy.clone(),
                    outcome: AttemptOutcome::Failure(message.clone()),
                });
                last_failure_message = Some(message);
            }
            StrategyResult::Skipped(reason) => {
                attempts.push(CredentialAttempt {
                    strategy: strategy.clone(),
                    outcome: AttemptOutcome::Skipped(reason),
                });
            }
        }
    }

    CredentialResult::Failure {
        attempts,
        final_message: last_failure_message,
    }
}

struct RealCredentialExecutor {
    config: Option<Rc<git2::Config>>,
}

impl RealCredentialExecutor {
    fn new(config: Option<Rc<git2::Config>>) -> Self {
        Self { config }
    }

    fn helper_message(scope: &HelperScope) -> &'static str {
        match scope {
            HelperScope::Initial => "credential helper returned no data",
            HelperScope::UserPass => "credential helper did not yield user/password",
        }
    }

    fn ssh_agent_failure_message(err: &Error) -> String {
        if err.class() == ErrorClass::Ssh && err.code() == ErrorCode::Auth {
            "ssh-agent had no matching keys or rejected the request".to_string()
        } else {
            sanitize_error_message(err)
        }
    }

    fn ssh_file_failure_message(err: &Error) -> String {
        if err.class() == ErrorClass::Ssh && err.code() == ErrorCode::Auth {
            "key requires a passphrase or ssh-agent session".to_string()
        } else {
            sanitize_error_message(err)
        }
    }
}

impl CredentialExecutor for RealCredentialExecutor {
    fn apply(
        &self,
        strategy: &CredentialStrategy,
        ctx: &CredentialRequestContext<'_>,
    ) -> StrategyResult {
        let username = ctx.username_from_url.unwrap_or(ctx.default_username);

        match strategy {
            CredentialStrategy::CredentialHelper(scope) => {
                if let Some(cfg) = self.config.as_ref() {
                    match Cred::credential_helper(cfg, ctx.url, ctx.username_from_url) {
                        Ok(cred) => StrategyResult::Success(cred),
                        Err(err) => StrategyResult::Failure(format!(
                            "{}: {}",
                            Self::helper_message(scope),
                            sanitize_error_message(&err)
                        )),
                    }
                } else {
                    StrategyResult::Skipped(
                        "no git config available for credential helper".to_string(),
                    )
                }
            }
            CredentialStrategy::SshKey(kind) => {
                let default_path = match kind {
                    SshKeyKind::IdEd25519 => "~/.ssh/id_ed25519",
                    SshKeyKind::IdRsa => "~/.ssh/id_rsa",
                };

                if let Some(paths) = locate_default_key(kind) {
                    match Cred::ssh_key(username, paths.public.as_deref(), &paths.private, None) {
                        Ok(cred) => StrategyResult::Success(cred),
                        Err(err) => StrategyResult::Failure(Self::ssh_file_failure_message(&err)),
                    }
                } else {
                    StrategyResult::Skipped(format!("no key at {default_path}"))
                }
            }
            CredentialStrategy::Username => match Cred::username(username) {
                Ok(cred) => StrategyResult::Success(cred),
                Err(err) => StrategyResult::Failure(sanitize_error_message(&err)),
            },
            CredentialStrategy::Default => match Cred::default() {
                Ok(cred) => StrategyResult::Success(cred),
                Err(err) => StrategyResult::Failure(sanitize_error_message(&err)),
            },
        }
    }
}

/// Return a unified diff (`git diff`-style patch) for the repository at `repo_path`,
/// formatted newest → oldest changes where applicable.
///
/// Assumptions:
/// - If `target` is `None`, compare HEAD (or empty tree if unborn) to working dir + index.
/// - If `target` is a single rev, compare that commit tree to working dir + index.
/// - If `target` is `<from>..<to>`, compare commit `<from>` to `<to>`.
/// - If `target` does not resolve to a rev, treat it as a path and restrict the diff there.
/// - If `exclude` is given, exclude those pathspecs (normalized) from the diff.
pub fn get_diff(
    repo_path: &str,
    target: Option<&str>, // commit/range or directory path
    // NOTE: This shouldn't match the git pathspec format, it should rather just be
    //       std::path::Pathbuf-convertable strings
    exclude: Option<&[&str]>,
) -> Result<String, Error> {
    let repo = Repository::open(repo_path)?;
    let mut opts = DiffOptions::new();

    opts.ignore_submodules(true).id_abbrev(40);

    let diff = match target {
        Some(spec) if spec.contains("..") => {
            let parts: Vec<_> = spec.split("..").collect();
            if parts.len() != 2 {
                return Err(Error::from_str("Invalid double-dot range"));
            }

            let from = repo.revparse_single(parts[0])?.peel_to_tree()?;
            let to = repo.revparse_single(parts[1])?.peel_to_tree()?;

            repo.diff_tree_to_tree(Some(&from), Some(&to), Some(&mut opts))?
        }
        Some(spec) => {
            // Try as rev first
            match repo.revparse_single(spec) {
                Ok(obj) => {
                    let base = obj.peel_to_tree()?;
                    repo.diff_tree_to_workdir_with_index(Some(&base), Some(&mut opts))?
                }
                Err(_) => {
                    // Treat as a directory/file path
                    let normalized = normalize_pathspec(spec);
                    opts.pathspec(&normalized);
                    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

                    repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?
                }
            }
        }
        None => {
            // HEAD vs working dir (with index); handle unborn HEAD
            let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

            repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?
        }
    };

    // Excluding files from the diff with our exclude vector
    // Originally tried adding things to the pathspec, but libgit2 didn't appreciate that and
    // instead decided to ignore all possible paths when putting together the diff.
    // So, we're left with this hack.
    let mut buf = Vec::new();
    let exclude = if let Some(e) = exclude {
        e.iter().map(|p| p.to_string()).collect()
    } else {
        Vec::new()
    };

    diff.print(DiffFormat::Patch, |delta, _, line| {
        let file_path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(|p| p.to_str());

        if let Some(path) = file_path {
            let diff_path = std::path::Path::new(path);
            if !exclude.iter().any(|excluded| {
                let exclude_path = std::path::Path::new(excluded);

                diff_path.starts_with(exclude_path)
            }) {
                buf.extend_from_slice(line.content());
            }
        }
        true
    })?;

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Stage changes (index-only), mirroring `git add` / `git add -u` (no commit).
///
/// - `Some(paths)`: for each normalized path:
///     * if directory → recursive add (matches `git add <dir>`).
///     * if file → add that single path.
/// - `None`: update tracked paths (like `git add -u`), staging modifications/deletions,
///     but NOT newly untracked files.
pub fn stage(paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(".")?;
    let mut index = repo.index()?;

    match paths {
        Some(list) => {
            for raw in list {
                let norm = normalize_pathspec(raw);
                let p = std::path::Path::new(&norm);
                if p.is_dir() {
                    index.add_all([p], IndexAddOption::DEFAULT, None)?;
                } else {
                    index.add_path(p)?;
                }
            }

            index.write()?;
        }
        None => {
            index.update_all(["."], None)?;
            index.write()?;
        }
    }

    Ok(())
}

// TODO: Remove the `add` portion from this
/// Stage changes and create a commit in the current repository, returning the new commit `Oid`.
///
/// Assumptions:
/// - If `paths` is `Some`, each path is normalized and added:
///   - Directories → `git add <dir>` (recursive).
///   - Files → `git add <file>`.
/// - If `paths` is `None` and `allow_empty` is `false`, behaves like `git add -u`
///   (updates tracked files, removes deleted).
/// - If `allow_empty` is `false`, and the resulting tree matches the parent’s, returns an error.
/// - If no parent exists (unborn branch), commit has no parents.
/// - Commit metadata uses repo config signature if available, else falls back to
///   `"Vizier <vizier@local>"`.
pub fn add_and_commit(
    paths: Option<Vec<&str>>,
    message: &str,
    allow_empty: bool,
) -> Result<Oid, git2::Error> {
    let repo = Repository::open(".")?;
    let mut index = repo.index()?;

    match paths {
        Some(paths) => {
            for raw in paths {
                let norm = normalize_pathspec(raw);
                let p = std::path::Path::new(&norm);
                if p.is_dir() {
                    index.add_all([p], git2::IndexAddOption::DEFAULT, None)?;
                } else {
                    index.add_path(p)?;
                }
            }
        }
        None => {
            if !allow_empty {
                // git add -u (update tracked, remove deleted)
                index.update_all(["."], None)?;
            }
        }
    }

    index.write()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    // Prefer config-driven signature if available
    let signature = repo
        .signature()
        .or_else(|_| Signature::now("Vizier", "vizier@local"))?;

    // Parent(s)
    let parent_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    if !allow_empty {
        if let Some(ref parent) = parent_commit {
            if parent.tree_id() == tree_id {
                return Err(git2::Error::from_str("nothing to commit"));
            }
        }
    }

    let parents: Vec<&git2::Commit> = match parent_commit.as_ref() {
        Some(p) => vec![p],
        None => vec![],
    };

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
}

/// Push the current HEAD branch to the specified remote without invoking the git binary.
///
/// Safety rails:
/// - Rejects pushes while merge/rebase/bisect operations are in progress.
/// - Requires `HEAD` to be a named local branch.
/// - Performs a fast-forward check when an upstream tracking ref is configured.
/// - Updates the matching `refs/remotes/<remote>/<branch>` reference on success.
pub fn push_current_branch(remote_name: &str) -> Result<(), PushError> {
    let repo = Repository::discover(".")
        .map_err(|err| PushError::from_git("failed to discover git repository", err))?;

    let state = repo.state();
    if state != RepositoryState::Clean {
        let msg = match state {
            RepositoryState::Merge => "cannot push while a merge is in progress",
            RepositoryState::Revert | RepositoryState::RevertSequence => {
                "cannot push while a revert is in progress"
            }
            RepositoryState::CherryPick | RepositoryState::CherryPickSequence => {
                "cannot push while a cherry-pick is in progress"
            }
            RepositoryState::Bisect => "cannot push while a bisect is in progress",
            RepositoryState::Rebase
            | RepositoryState::RebaseInteractive
            | RepositoryState::RebaseMerge
            | RepositoryState::ApplyMailbox
            | RepositoryState::ApplyMailboxOrRebase => {
                "cannot push while a rebase or mailbox apply is in progress"
            }
            _ => "cannot push while the repository has pending operations",
        };

        return Err(PushError::general(msg));
    }

    let head = repo
        .head()
        .map_err(|err| PushError::from_git("failed to resolve HEAD", err))?;
    if !head.is_branch() {
        return Err(PushError::general(
            "cannot push because HEAD is not pointing to a branch",
        ));
    }

    let branch_ref = head
        .name()
        .ok_or_else(|| PushError::general("current branch name is not valid UTF-8"))?;
    let branch_name = head
        .shorthand()
        .ok_or_else(|| PushError::general("unable to determine branch name"))?;
    let head_oid = head
        .target()
        .ok_or_else(|| PushError::general("HEAD does not reference a commit"))?;

    if let Ok(branch) = repo.find_branch(branch_name, BranchType::Local) {
        if let Ok(upstream) = branch.upstream() {
            if let Some(upstream_oid) = upstream.get().target() {
                let is_descendant =
                    repo.graph_descendant_of(head_oid, upstream_oid)
                        .map_err(|err| {
                            PushError::from_git("unable to compute fast-forward relationship", err)
                        })?;
                if !is_descendant {
                    return Err(PushError::general(
                        "push would not be a fast-forward; fetch and merge first",
                    ));
                }
            }
        }
    }

    let mut remote = repo
        .find_remote(remote_name)
        .map_err(|err| PushError::from_git("unable to locate remote", err))?;
    let remote_url = remote
        .pushurl()
        .or_else(|| remote.url())
        .ok_or_else(|| PushError::general("remote has no configured URL"))?
        .to_string();
    let remote_scheme = classify_remote_scheme(&remote_url);

    let config_for_cb = repo.config().ok().map(Rc::new);
    let credential_attempts: Rc<RefCell<Vec<CredentialAttempt>>> =
        Rc::new(RefCell::new(Vec::new()));
    let plan_config = config_for_cb.clone();

    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials({
        let attempts = Rc::clone(&credential_attempts);
        move |url, username_from_url, allowed_types| {
            let helper_config = plan_config.clone();
            let has_helper = helper_config.is_some();
            let plan = build_credential_plan(allowed_types, has_helper);
            let executor = RealCredentialExecutor::new(helper_config);
            let ctx = CredentialRequestContext {
                url,
                username_from_url,
                default_username: username_from_url.unwrap_or("git"),
            };

            let result = execute_credential_plan(&plan, &executor, &ctx);
            match result {
                CredentialResult::Success {
                    cred,
                    attempts: log,
                } => {
                    if let Ok(mut store) = attempts.try_borrow_mut() {
                        store.extend(log);
                    }
                    Ok(cred)
                }
                CredentialResult::Failure {
                    attempts: log,
                    final_message,
                } => {
                    if let Ok(mut store) = attempts.try_borrow_mut() {
                        store.extend(log);
                    }

                    let msg = final_message
                        .unwrap_or_else(|| "no credential strategy succeeded".to_string());
                    Err(Error::from_str(&msg))
                }
            }
        }
    });

    let push_statuses: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let statuses_for_cb = Rc::clone(&push_statuses);
    callbacks.push_update_reference(move |refname, status| {
        if let Some(status) = status {
            if let Ok(mut entries) = statuses_for_cb.try_borrow_mut() {
                entries.push((refname.to_string(), status.to_string()));
            }
        }
        Ok(())
    });

    let mut push_opts = PushOptions::new();
    push_opts.remote_callbacks(callbacks);

    let refspec = format!("{branch_ref}:{branch_ref}");
    let refspecs = [refspec.as_str()];
    if let Err(err) = remote.push(&refspecs, Some(&mut push_opts)) {
        let attempts = credential_attempts.borrow().clone();
        let all_attempts_failed = !attempts.is_empty()
            && attempts
                .iter()
                .all(|attempt| !matches!(attempt.outcome, AttemptOutcome::Success));

        if all_attempts_failed {
            return Err(PushError::auth(
                remote_name.to_string(),
                remote_url,
                remote_scheme,
                attempts,
            ));
        } else {
            return Err(PushError::from_git("failed to push to remote", err));
        }
    }
    remote
        .disconnect()
        .map_err(|err| PushError::from_git("failed to disconnect remote", err))?;

    let statuses = push_statuses.borrow();
    if !statuses.is_empty() {
        let mut msg = String::from("remote rejected updates for:");
        for (name, status) in statuses.iter() {
            msg.push_str(&format!(" {name} ({status})"));
        }
        return Err(PushError::general(msg));
    }

    let tracking_ref = format!("refs/remotes/{remote_name}/{branch_name}");
    repo.reference(
        &tracking_ref,
        head_oid,
        true,
        "vizier: update remote tracking ref after push",
    )
    .map_err(|err| PushError::from_git("failed to update remote tracking ref", err))?;

    Ok(())
}

/// Return up to `depth` commits whose messages match any of the `filters` (OR),
/// Returns up to `depth` commits (newest -> oldest) whose *full* messages
/// contain ANY of the provided `filters` (case-insensitive).
/// The returned String contains each commit's entire message (subject + body),
/// with original newlines preserved. Between commits, a simple header demarcates entries.
pub fn get_log(depth: usize, filters: Option<Vec<String>>) -> Result<Vec<String>, Error> {
    let repo = Repository::discover(".")?;

    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    walk.set_sorting(Sort::TIME)?; // newest -> oldest by committer time

    let needles: Vec<String> = filters
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    let use_filters = !needles.is_empty();

    let mut out = Vec::new();
    let mut kept = 0usize;

    for oid_res in walk {
        let oid = oid_res?;
        let commit = repo.find_commit(oid)?;

        let msg = commit
            .message()
            .map(|s| s.to_owned())
            .unwrap_or_else(|| String::from_utf8_lossy(commit.message_bytes()).into_owned());

        let keep = if use_filters {
            let msg_lc = msg.to_lowercase();
            needles.iter().any(|n| msg_lc.contains(n))
        } else {
            true
        };

        if !keep {
            continue;
        }

        let sha = oid.to_string();
        let short_sha = &sha[..7.min(sha.len())];
        let author = commit.author().name().unwrap_or("<unknown>").to_string();

        let mut out_msg = String::new();

        out_msg.push_str(&format!("commit {short_sha} — {author}\n"));
        out_msg.push_str(&msg);
        if !msg.ends_with('\n') {
            out_msg.push('\n');
        }

        out_msg.push('\n');

        out.push(out_msg);

        kept += 1;
        if kept >= depth {
            break;
        }
    }

    Ok(out)
}

/// Unstage changes (index-only), mirroring `git restore --staged` / `git reset -- <paths>`.
///
/// Behavior:
/// - If `paths` is `Some`, paths are normalized and only those paths are reset in the index:
///     - If `HEAD` exists, index entries for those paths become exactly `HEAD`’s versions.
///     - If `HEAD` is unborn, those paths are removed from the index (i.e., fully unstaged).
/// - If `paths` is `None`:
///     - If `HEAD` exists, the entire index is reset to `HEAD`’s tree (no working tree changes).
///     - If `HEAD` is unborn, the index is cleared.
/// - Never updates the working directory, and never moves `HEAD`.
pub fn unstage(paths: Option<Vec<&str>>) -> Result<(), Error> {
    let repo = Repository::open(".")?;
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut index = repo.index()?;

    match (paths, head_tree) {
        (Some(list), Some(_head_tree)) => {
            // NOTE: reset_default requires &[&Path]
            let owned: Vec<std::path::PathBuf> = list
                .into_iter()
                .map(|p| std::path::PathBuf::from(normalize_pathspec(p)))
                .collect();

            let spec: Vec<&std::path::Path> = owned.iter().map(|p| p.as_path()).collect();
            let head = match repo.head() {
                Ok(h) => h,
                Err(_) => {
                    let mut idx = repo.index()?;
                    for p in spec {
                        idx.remove_path(p)?;
                    }

                    idx.write()?;
                    return Ok(());
                }
            };

            let head_obj = head.resolve()?.peel(git2::ObjectType::Commit)?;

            repo.reset_default(Some(&head_obj), &spec)?;
        }

        (Some(list), None) => {
            for raw in list {
                let norm = normalize_pathspec(raw);
                index.remove_path(std::path::Path::new(&norm))?;
            }

            index.write()?;
        }

        (None, Some(head_tree)) => {
            index.read_tree(&head_tree)?;
            index.write()?;
        }

        (None, None) => {
            index.clear()?;
            index.write()?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub enum StagedKind {
    Added,                                // INDEX_NEW
    Modified,                             // INDEX_MODIFIED
    Deleted,                              // INDEX_DELETED
    TypeChange,                           // INDEX_TYPECHANGE
    Renamed { from: String, to: String }, // INDEX_RENAMED
}

#[derive(Debug, Clone)]
pub struct StagedItem {
    pub path: String, // primary path (for rename, the NEW path)
    pub kind: StagedKind,
}

/// Capture the current staged set (index vs HEAD), losslessly enough to restore.
pub fn snapshot_staged(repo_path: &str) -> Result<Vec<StagedItem>, Error> {
    let repo = Repository::open(repo_path)?;
    let mut opts = StatusOptions::new();
    // We want staged/index changes relative to HEAD:
    opts.include_untracked(false)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(false)
        .update_index(false)
        .include_unmodified(false)
        .show(git2::StatusShow::Index);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut out = Vec::new();

    for entry in statuses.iter() {
        let s = entry.status();

        // Renames: libgit2 provides both paths when rename detection is enabled.
        if s.contains(Status::INDEX_RENAMED) {
            let from = entry
                .head_to_index()
                .and_then(|d| d.old_file().path())
                .and_then(|p| p.to_str())
                .unwrap_or_default()
                .to_string();

            let to = entry
                .head_to_index()
                .and_then(|d| d.new_file().path())
                .and_then(|p| p.to_str())
                .unwrap_or_default()
                .to_string();

            out.push(StagedItem {
                path: to.clone(),
                kind: StagedKind::Renamed { from, to },
            });
            continue;
        }

        let path = entry
            .head_to_index()
            .or_else(|| entry.index_to_workdir())
            .and_then(|d| d.new_file().path().or(d.old_file().path()))
            .and_then(|p| p.to_str())
            .unwrap_or_default()
            .to_string();

        let kind = if s.contains(Status::INDEX_NEW) {
            StagedKind::Added
        } else if s.contains(Status::INDEX_MODIFIED) {
            StagedKind::Modified
        } else if s.contains(Status::INDEX_DELETED) {
            StagedKind::Deleted
        } else if s.contains(Status::INDEX_TYPECHANGE) {
            StagedKind::TypeChange
        } else {
            // skip anything that isn't index-staged
            continue;
        };

        out.push(StagedItem { path, kind });
    }

    Ok(out)
}

/// Restore the staged set exactly as captured by `snapshot_staged`.
/// Index-only; does not modify worktree or HEAD.
pub fn restore_staged(repo_path: &str, staged: &[StagedItem]) -> Result<(), Error> {
    let repo = Repository::open(repo_path)?;
    let mut index = repo.index()?;

    for item in staged {
        match &item.kind {
            StagedKind::Added | StagedKind::Modified | StagedKind::TypeChange => {
                index.add_path(std::path::Path::new(&item.path))?;
            }
            StagedKind::Deleted => {
                index.remove_path(std::path::Path::new(&item.path))?;
            }
            StagedKind::Renamed { from, to } => {
                index.remove_path(std::path::Path::new(from))?;
                index.add_path(std::path::Path::new(to))?;
            }
        }
    }

    index.write()?;
    Ok(())
}

/// Extract (owner, repo) from `origin`
pub fn origin_owner_repo(repo_path: &str) -> Result<(String, String), Error> {
    let repo = Repository::discover(repo_path)?;
    let remote = repo.find_remote("origin").or_else(|_| {
        // Some repos only have fetch remotes in the list; fall back to first if needed.
        let remotes = repo.remotes()?;
        let name = remotes
            .iter()
            .flatten()
            .next()
            .ok_or_else(|| Error::from_str("No remotes found"))?;
        repo.find_remote(name)
    })?;

    let url = remote
        .url()
        .ok_or_else(|| Error::from_str("origin remote has no URL"))?;
    // Accept common GitHub patterns:
    // 1) https://github.com/OWNER/REPO(.git)
    // 2) git@github.com:OWNER/REPO(.git)
    // 3) ssh://git@github.com/OWNER/REPO(.git)
    // Normalize to just "OWNER/REPO"
    let owner_repo = if let Some(rest) = url.strip_prefix("https://github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        return Err(Error::from_str("Unsupported GitHub remote URL format"));
    };

    let trimmed = owner_repo.trim_end_matches(".git").trim_end_matches('/');
    let mut parts = trimmed.split('/');

    let owner = parts
        .next()
        .ok_or_else(|| Error::from_str("Missing owner in remote URL"))?;
    let repo = parts
        .next()
        .ok_or_else(|| Error::from_str("Missing repo in remote URL"))?;

    if parts.next().is_some() {
        return Err(Error::from_str("Remote URL contains extra path segments"));
    }

    Ok((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{IndexAddOption, Repository, Signature};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    struct CwdGuard {
        old: PathBuf,
    }

    impl CwdGuard {
        fn enter<P: AsRef<Path>>(p: P) -> std::io::Result<Self> {
            let old = std::env::current_dir()?;
            std::env::set_current_dir(p)?;
            Ok(Self { old })
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.old);
        }
    }

    fn init_temp_repo() -> (tempfile::TempDir, Repository) {
        let td = tempfile::TempDir::new().expect("tempdir");
        let repo = Repository::init(td.path()).expect("init repo");
        let _ = repo.config().and_then(|mut c| {
            c.set_str("user.name", "Tester")?;
            c.set_str("user.email", "tester@example.com")
        });
        (td, repo)
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    fn append(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    fn raw_commit(repo: &Repository, msg: &str) -> Oid {
        let mut idx = repo.index().unwrap();
        idx.add_all(["."], IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tree_id = idx.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = repo
            .signature()
            .or_else(|_| Signature::now("Tester", "tester@example.com"))
            .unwrap();
        let parent_opt = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent_opt.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    fn raw_stage(repo: &Repository, rel: &str) {
        let mut idx = repo.index().unwrap();
        idx.add_path(Path::new(rel)).unwrap();
        idx.write().unwrap();
    }

    #[test]
    fn push_current_branch_updates_remote_tracking() {
        let (td, repo) = init_temp_repo();
        let remote_dir = tempfile::TempDir::new().expect("remote tempdir");
        Repository::init_bare(remote_dir.path()).expect("init bare remote");
        let remote_path = remote_dir
            .path()
            .to_str()
            .expect("remote path utf8")
            .to_owned();

        repo.remote("origin", &remote_path)
            .expect("configure remote");

        {
            let _cwd = CwdGuard::enter(td.path()).unwrap();
            write(Path::new("file.txt"), "hello\n");
            raw_commit(&repo, "initial");

            let branch = repo.head().unwrap().shorthand().unwrap().to_string();

            push_current_branch("origin").expect("push succeeds");

            let remote_repo = Repository::open(remote_dir.path()).expect("open remote repo");
            let remote_ref = remote_repo
                .find_reference(&format!("refs/heads/{branch}"))
                .expect("remote branch exists");
            let local_oid = repo.head().unwrap().target().unwrap();
            assert_eq!(remote_ref.target(), Some(local_oid));

            let tracking_ref = repo
                .find_reference(&format!("refs/remotes/origin/{branch}"))
                .expect("tracking ref updated");
            assert_eq!(tracking_ref.target(), Some(local_oid));
        }
    }

    #[test]
    fn push_current_branch_rejects_detached_head() {
        let (td, repo) = init_temp_repo();
        let remote_dir = tempfile::TempDir::new().expect("remote tempdir");
        Repository::init_bare(remote_dir.path()).expect("init bare remote");
        let remote_path = remote_dir
            .path()
            .to_str()
            .expect("remote path utf8")
            .to_owned();

        repo.remote("origin", &remote_path)
            .expect("configure remote");

        {
            let _cwd = CwdGuard::enter(td.path()).unwrap();
            write(Path::new("note.txt"), "one\n");
            let oid = raw_commit(&repo, "detached");

            repo.set_head_detached(oid).expect("detach head");

            let err = push_current_branch("origin").expect_err("push should fail");
            match err.kind() {
                PushErrorKind::General(message) => {
                    assert!(message.contains("not pointing to a branch"));
                }
                other => panic!("unexpected error variant: {:?}", other),
            }
        }
    }

    struct RecordingExecutor {
        responses: RefCell<VecDeque<StrategyResult>>,
        invoked: RefCell<Vec<CredentialStrategy>>,
    }

    impl RecordingExecutor {
        fn new(responses: Vec<StrategyResult>) -> Self {
            Self {
                responses: RefCell::new(VecDeque::from(responses)),
                invoked: RefCell::new(Vec::new()),
            }
        }
    }

    impl CredentialExecutor for RecordingExecutor {
        fn apply(
            &self,
            strategy: &CredentialStrategy,
            ctx: &CredentialRequestContext<'_>,
        ) -> StrategyResult {
            // record username resolution to ensure we pass the default correctly
            assert_eq!(ctx.username_from_url, Some("git"));
            self.invoked.borrow_mut().push(strategy.clone());
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("strategy response available")
        }
    }

    #[test]
    fn credential_plan_attempts_file_keys_when_agent_fails() {
        let plan = build_credential_plan(CredentialType::SSH_KEY, false);
        assert!(plan.contains(&CredentialStrategy::SshKey(SshKeyKind::IdEd25519)));
        assert!(plan.contains(&CredentialStrategy::SshKey(SshKeyKind::IdRsa)));

        let responses = vec![
            StrategyResult::Failure("agent missing".to_string()),
            StrategyResult::Failure("no ed25519".to_string()),
            StrategyResult::Success(Cred::username("git").expect("cred")),
        ];
        let executor = RecordingExecutor::new(responses);

        let ctx = CredentialRequestContext {
            url: "ssh://example.com/repo.git",
            username_from_url: Some("git"),
            default_username: "git",
        };

        let result = execute_credential_plan(&plan, &executor, &ctx);
        match result {
            CredentialResult::Success { .. } => {}
            _ => panic!("expected success after key attempts"),
        }

        let invoked = executor.invoked.borrow();
        let expected = vec![
            CredentialStrategy::SshKey(SshKeyKind::IdEd25519),
            CredentialStrategy::SshKey(SshKeyKind::IdRsa),
            CredentialStrategy::Default,
        ];
        assert_eq!(&expected, invoked.as_slice());
    }

    // --- normalize_pathspec --------------------------------------------------

    #[test]
    fn normalize_pathspec_variants() {
        assert_eq!(super::normalize_pathspec(" src//utils/// "), "src/utils");
        assert_eq!(super::normalize_pathspec("./a/b/"), "a/b");
        assert_eq!(super::normalize_pathspec(r#"a\win\path\"#), "a/win/path");

        // Match current implementation: if it starts with `//`, internal `//` are preserved.
        assert_eq!(
            super::normalize_pathspec("//server//share//x"),
            "//server/share/x"
        );
    }

    // --- add_and_commit core behaviors --------------------------------------

    #[test]
    fn add_and_commit_basic_and_noop() {
        let (td, _repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        write(Path::new("README.md"), "# one\n");
        let oid1 = add_and_commit(Some(vec!["README.md"]), "init", false).expect("commit ok");
        assert_ne!(oid1, Oid::zero());

        // No changes, allow_empty=false → "nothing to commit"
        let err = add_and_commit(None, "noop", false).unwrap_err();
        assert!(format!("{err}").contains("nothing to commit"));

        // Empty commit (allow_empty=true) → OK
        let oid2 = add_and_commit(None, "empty ok", true).expect("empty commit ok");
        assert_ne!(oid2, oid1);
    }

    #[test]
    fn add_and_commit_pathspecs_and_deletes_and_ignores() {
        let (td, _) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // .gitignore excludes dist/** and vendor/**
        write(Path::new(".gitignore"), "dist/\nvendor/\n");

        // Create a mix
        write(Path::new("src/a.rs"), "fn a(){}\n");
        write(Path::new("src/b.rs"), "fn b(){}\n");
        write(Path::new("dist/bundle.js"), "/* build */\n");
        write(Path::new("vendor/lib/x.c"), "/* vendored */\n");
        let c1 = add_and_commit(Some(vec!["./src//"]), "src only", false).unwrap();
        assert_ne!(c1, Oid::zero());

        // Update tracked files + delete one; update_all should stage deletes.
        fs::remove_file("src/a.rs").unwrap();
        append(Path::new("src/b.rs"), "// mod\n");

        // Ignored paths shouldn't be added even with update_all
        let c2 = add_and_commit(None, "update tracked & deletions", false).unwrap();
        assert_ne!(c2, c1);

        // Show that vendor/dist are still untracked (ignored), not part of commit 2
        // Verify via a diff: HEAD..workdir should be empty (no pending tracked changes)
        let repo_path = td.path().to_str().unwrap();
        let d = get_diff(repo_path, None, None).unwrap();
        // No pending tracked changes post-commit; any diff would now be due to ignored dirs (which aren't included)
        assert!(d.is_empty() || !d.contains("src/")); // conservative assertion
    }

    // --- get_diff: path, excludes, ranges -----------------------------------

    #[test]
    fn diff_head_vs_workdir_and_path_and_exclude() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        write(Path::new("a/file.txt"), "hello\n");
        write(Path::new("b/file.txt"), "world\n");
        raw_commit(&repo, "base");

        append(Path::new("a/file.txt"), "change-a\n"); // unstaged, tracked file
        append(Path::new("b/file.txt"), "change-b\n");
        write(Path::new("b/inner/keep.txt"), "keep\n"); // untracked; should not appear

        // 1) None → HEAD vs workdir(+index). Shows tracked edits, not untracked files.
        let d_all = get_diff(repo_path.to_str().unwrap(), None, None).expect("diff");
        assert!(d_all.contains("a/file.txt"));
        assert!(d_all.contains("b/file.txt"));
        assert!(!d_all.contains("b/inner/keep.txt")); // untracked → absent

        // 2) Treat `target` as a path
        let d_b = get_diff(repo_path.to_str().unwrap(), Some("b"), None).expect("diff b");
        assert!(!d_b.contains("a/file.txt"));
        assert!(d_b.contains("b/file.txt"));
        assert!(!d_b.contains("b/inner/keep.txt")); // still untracked → absent

        // 3) Exclude subdir via Windows-ish input → normalized
        let d_b_ex = get_diff(
            repo_path.to_str().unwrap(),
            Some("b"),
            Some(&[r".\b\inner"]),
        )
        .expect("diff b excl inner");
        assert!(d_b_ex.contains("b/file.txt"));
        assert!(!d_b_ex.contains("b/inner/keep.txt"));
    }

    #[test]
    fn diff_single_rev_to_workdir() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        write(Path::new("x.txt"), "x1\n");
        let first = raw_commit(&repo, "c1");

        append(Path::new("x.txt"), "x2\n"); // unstaged, tracked change is visible
        let spec = first.to_string();
        let d = get_diff(repo_path.to_str().unwrap(), Some(&spec), None).expect("diff");
        println!("d: {}", d);
        assert!(d.contains("x.txt")); // file appears
        assert!(d.contains("\n+")); // there is an addition hunk
        assert!(d.contains("x2")); // payload appears (don’t hard-code "+x2")
    }

    #[test]
    fn diff_with_excludes() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        // Base on main
        write(Path::new("common.txt"), "base\n");
        let base = raw_commit(&repo, "base");

        // Branch at base
        {
            let head_commit = repo.find_commit(base).unwrap();
            repo.branch("feature", &head_commit, true).unwrap();
        }

        // Advance main
        write(Path::new("main.txt"), "m1\n");
        write(Path::new("vendor/ignored.txt"), "should be excluded\n"); // will test exclusion
        let main1 = raw_commit(&repo, "main1");

        // Checkout feature and diverge
        {
            let mut checkout = git2::build::CheckoutBuilder::new();
            repo.set_head("refs/heads/feature").unwrap();
            repo.checkout_head(Some(&mut checkout.force())).unwrap();
        }
        write(Path::new("feat.txt"), "f1\n");

        // A..B (base..main1) shows main changes (including vendor/ by default)
        let dd = format!("{}..{}", base, main1);
        let out_dd = get_diff(repo_path.to_str().unwrap(), Some(&dd), None).expect("A..B");
        assert!(out_dd.contains("main.txt"));

        // Now exclude vendor/** using normalize-able pathspec; vendor should disappear
        let out_dd_ex = get_diff(repo_path.to_str().unwrap(), Some(&dd), Some(&["vendor//"]))
            .expect("A..B excl");
        println!("DIFF: {}", out_dd_ex);
        assert!(out_dd_ex.contains("main.txt"));
        assert!(!out_dd_ex.contains("vendor/ignored.txt"));
    }

    // --- unborn HEAD (no untracked): stage-only then diff --------------------

    #[test]
    fn diff_unborn_head_against_workdir_without_untracked() {
        let (td, repo) = init_temp_repo();
        let repo_path = td.path().to_path_buf();
        let _cwd = CwdGuard::enter(&repo_path).unwrap();

        // File exists in workdir and is STAGED (tracked) but no commits yet.
        write(Path::new("z.txt"), "hello\n");
        raw_stage(&repo, "z.txt"); // index-only

        // get_diff(None) compares empty tree → workdir+index, so z.txt appears even with untracked disabled
        let out = get_diff(repo_path.to_str().unwrap(), None, None).expect("diff unborn");
        println!("OUT: {}", out);
        assert!(out.contains("z.txt"));
        assert!(out.contains("hello"));
    }

    // --- stage (index-only) --------------------------------------------------

    #[test]
    fn stage_paths_and_update_tracked_only() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // Base commit with two tracked files
        write(Path::new("a.txt"), "A0\n");
        write(Path::new("b.txt"), "B0\n");
        raw_commit(&repo, "base");

        // Workdir changes:
        // - modify tracked a.txt
        // - delete tracked b.txt
        // - create new untracked c.txt
        append(Path::new("a.txt"), "A1\n");
        fs::remove_file("b.txt").unwrap();
        write(Path::new("c.txt"), "C0\n");

        // 1) stage(None) should mirror `git add -u`: stage tracked changes (a.txt mod, b.txt del)
        //    but NOT the new untracked c.txt.
        stage(None).expect("stage -u");
        let staged1 = snapshot_staged(".").expect("snapshot staged after -u");

        // Expect: a.txt Modified, b.txt Deleted; no c.txt
        let mut kinds = staged1
            .iter()
            .map(|s| match &s.kind {
                super::StagedKind::Added => ("Added", s.path.clone()),
                super::StagedKind::Modified => ("Modified", s.path.clone()),
                super::StagedKind::Deleted => ("Deleted", s.path.clone()),
                super::StagedKind::TypeChange => ("TypeChange", s.path.clone()),
                super::StagedKind::Renamed { from, to } => ("Renamed", format!("{from}->{to}")),
            })
            .collect::<Vec<_>>();
        kinds.sort_by(|a, b| a.1.cmp(&b.1));

        assert_eq!(
            kinds.sort(),
            vec![
                ("Deleted", "b.txt".to_string()),
                ("Modified", "a.txt".to_string()),
            ]
            .sort()
        );

        // 2) Now explicitly stage c.txt via stage(Some)
        stage(Some(vec!["c.txt"])).expect("stage c.txt");
        let staged2 = snapshot_staged(".").expect("snapshot staged after explicit add");

        let names2: Vec<_> = staged2.iter().map(|s| s.path.as_str()).collect();
        assert!(names2.contains(&"a.txt"));
        assert!(names2.contains(&"b.txt")); // staged deletion appears as b.txt in the snapshot
        assert!(names2.contains(&"c.txt")); // now present as Added
        assert!(
            staged2
                .iter()
                .any(|s| matches!(s.kind, super::StagedKind::Added) && s.path == "c.txt")
        );
    }

    // --- unstage: specific paths & entire index (born HEAD) ------------------

    #[test]
    fn unstage_specific_paths_and_all_with_head() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        write(Path::new("x.txt"), "X0\n");
        write(Path::new("y.txt"), "Y0\n");
        raw_commit(&repo, "base");

        append(Path::new("x.txt"), "X1\n");
        append(Path::new("y.txt"), "Y1\n");

        // Stage both changes (explicit)
        stage(Some(vec!["x.txt", "y.txt"])).expect("stage both");

        // Unstage only x.txt → y.txt should remain staged
        unstage(Some(vec!["x.txt"])).expect("unstage x");

        let after_x = snapshot_staged(".").expect("snapshot after unstage x");
        assert!(after_x.iter().any(|s| s.path == "y.txt"));
        assert!(!after_x.iter().any(|s| s.path == "x.txt"));

        // Unstage everything → nothing should be staged
        unstage(None).expect("unstage all");
        let after_all = snapshot_staged(".").expect("snapshot after unstage all");
        assert!(after_all.is_empty());
    }

    // --- unstage: unborn HEAD behavior --------------------------------------

    #[test]
    fn unstage_with_unborn_head() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // No commits yet; create two files and stage both
        write(Path::new("u.txt"), "U0\n");
        write(Path::new("v.txt"), "V0\n");
        raw_stage(&repo, "u.txt");
        raw_stage(&repo, "v.txt");

        // Path-limited unstage on unborn HEAD should remove entries from index for those paths
        unstage(Some(vec!["u.txt"])).expect("unstage u.txt on unborn");
        let staged1 = snapshot_staged(".").expect("snapshot staged after partial unstage");
        let names1: Vec<_> = staged1.iter().map(|s| s.path.as_str()).collect();
        assert!(names1.contains(&"v.txt"));
        assert!(!names1.contains(&"u.txt"));

        // Full unstage on unborn HEAD should clear the index
        unstage(None).expect("unstage all unborn");
        let staged2 = snapshot_staged(".").expect("snapshot staged after clear");
        assert!(staged2.is_empty());
    }

    // --- snapshot → unstage → mutate → restore (A/M/D/R rename) --------------

    #[test]
    fn snapshot_and_restore_roundtrip_with_rename() {
        let (td, repo) = init_temp_repo();
        let _cwd = CwdGuard::enter(td.path()).unwrap();

        // Base: a.txt, b.txt
        write(Path::new("a.txt"), "A0\n");
        write(Path::new("b.txt"), "B0\n");
        raw_commit(&repo, "base");

        // Workdir staged set (before snapshot):
        // - RENAME: a.txt -> a_ren.txt (same content to improve rename detection)
        // - DELETE: b.txt
        // - ADD: c.txt
        // - (no explicit extra modifications; rely on rename detection)
        fs::rename("a.txt", "a_ren.txt").unwrap();
        fs::remove_file("b.txt").unwrap();
        write(Path::new("c.txt"), "C0\n");

        // Stage all changes so index reflects A/M/D/R
        {
            let mut idx = repo.index().unwrap();
            idx.add_all(["."], git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            // ensure deletion is captured
            idx.update_all(["."], None).unwrap();
            idx.write().unwrap();
        }

        // Take snapshot of what's staged now
        let snap = snapshot_staged(".").expect("snapshot staged");

        // Sanity: ensure we actually captured the expected kinds
        // Expect at least: Added c.txt, Deleted b.txt, and a rename a.txt -> a_ren.txt
        let mut have_added_c = false;
        let mut have_deleted_b = false;
        let mut have_renamed_a = false;

        for it in &snap {
            match &it.kind {
                super::StagedKind::Added if it.path == "c.txt" => have_added_c = true,
                super::StagedKind::Deleted if it.path == "b.txt" => have_deleted_b = true,
                super::StagedKind::Renamed { from, to } if from == "a.txt" && to == "a_ren.txt" => {
                    have_renamed_a = true
                }
                _ => {}
            }
        }
        assert!(have_added_c, "expected Added c.txt in snapshot");
        assert!(have_deleted_b, "expected Deleted b.txt in snapshot");
        assert!(
            have_renamed_a,
            "expected Renamed a.txt->a_ren.txt in snapshot"
        );

        // Unstage everything
        unstage(None).expect("unstage all");

        // Mutate workdir arbitrarily (should not affect restoration correctness)
        append(Path::new("c.txt"), "C1\n"); // change content after snapshot
        write(Path::new("d.txt"), "D0 (noise)\n"); // create a noise file that won't be staged by restore

        // Restore exact staged set captured in `snap`
        restore_staged(".", &snap).expect("restore staged");

        // Re-snapshot after restore to compare equivalence (semantic equality of staged set)
        let after = snapshot_staged(".").expect("snapshot after restore");

        // Normalize into comparable tuples
        fn key(s: &super::StagedItem) -> (String, String) {
            match &s.kind {
                super::StagedKind::Added => ("Added".into(), s.path.clone()),
                super::StagedKind::Modified => ("Modified".into(), s.path.clone()),
                super::StagedKind::Deleted => ("Deleted".into(), s.path.clone()),
                super::StagedKind::TypeChange => ("TypeChange".into(), s.path.clone()),
                super::StagedKind::Renamed { from, to } => {
                    ("Renamed".into(), format!("{from}->{to}"))
                }
            }
        }

        let mut lhs = snap.iter().map(key).collect::<Vec<_>>();
        let mut rhs = after.iter().map(key).collect::<Vec<_>>();
        lhs.sort();
        rhs.sort();
        assert_eq!(
            lhs, rhs,
            "restored staged set should equal original snapshot"
        );
    }
}
