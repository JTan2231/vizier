use git2::{
    BranchType, Cred, CredentialType, Error, ErrorClass, ErrorCode, PushOptions, RemoteCallbacks,
    Repository, RepositoryState,
};
use std::cell::RefCell;
use std::env;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

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
    if let Some(home) = env::var_os("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home));
    }

    #[cfg(windows)]
    {
        if let Some(profile) = env::var_os("USERPROFILE")
            && !profile.is_empty()
        {
            return Some(PathBuf::from(profile));
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

pub(crate) fn build_credential_plan(
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

pub(crate) struct CredentialRequestContext<'a> {
    pub(crate) url: &'a str,
    pub(crate) username_from_url: Option<&'a str>,
    pub(crate) default_username: &'a str,
}

pub(crate) enum StrategyResult {
    Success(Cred),
    Failure(String),
    Skipped(String),
}

pub(crate) enum CredentialResult {
    Success {
        cred: Cred,
        attempts: Vec<CredentialAttempt>,
    },
    Failure {
        attempts: Vec<CredentialAttempt>,
        final_message: Option<String>,
    },
}

pub(crate) trait CredentialExecutor {
    fn apply(
        &self,
        strategy: &CredentialStrategy,
        ctx: &CredentialRequestContext<'_>,
    ) -> StrategyResult;
}

pub(crate) fn execute_credential_plan<E: CredentialExecutor>(
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

/// Push the current HEAD branch to the specified remote without invoking the git binary.
///
/// Safety rails:
/// - Rejects pushes while merge/rebase/bisect operations are in progress.
/// - Requires `HEAD` to be a named local branch.
/// - Performs a fast-forward check when an upstream tracking ref is configured.
/// - Updates the matching `refs/remotes/<remote>/<branch>` reference on success.
fn push_current_branch_impl(repo: &Repository, remote_name: &str) -> Result<(), PushError> {
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

    if let Ok(branch) = repo.find_branch(branch_name, BranchType::Local)
        && let Ok(upstream) = branch.upstream()
        && let Some(upstream_oid) = upstream.get().target()
    {
        let is_descendant = repo
            .graph_descendant_of(head_oid, upstream_oid)
            .map_err(|err| {
                PushError::from_git("unable to compute fast-forward relationship", err)
            })?;
        if !is_descendant {
            return Err(PushError::general(
                "push would not be a fast-forward; fetch and merge first",
            ));
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
        if let Some(status) = status
            && let Ok(mut entries) = statuses_for_cb.try_borrow_mut()
        {
            entries.push((refname.to_string(), status.to_string()));
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

pub fn push_current_branch(remote_name: &str) -> Result<(), PushError> {
    let repo = Repository::discover(".")
        .map_err(|err| PushError::from_git("failed to discover git repository", err))?;
    push_current_branch_impl(&repo, remote_name)
}

pub fn push_current_branch_in<P: AsRef<std::path::Path>>(
    repo_path: P,
    remote_name: &str,
) -> Result<(), PushError> {
    let repo = Repository::discover(repo_path)
        .map_err(|err| PushError::from_git("failed to discover git repository", err))?;
    push_current_branch_impl(&repo, remote_name)
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
