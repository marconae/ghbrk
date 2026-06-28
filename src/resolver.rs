use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::policy::Operation;

const GITHUB_HOST: &str = "github.com";
const WILDCARD: &str = "*";

/// Outcome of resolving a shim invocation into a policy-engine input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub org: String,
    pub repo: String,
    pub branch: Option<String>,
    pub operation: Operation,
    pub url_scheme: UrlScheme,
}

/// Transport scheme of the remote URL the resolver inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlScheme {
    Ssh,
    Https,
}

/// Parsed GitHub remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRemote {
    pub org: String,
    pub repo: String,
    pub scheme: UrlScheme,
}

/// All errors the resolver can produce.
#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("remote host '{0}' is not GitHub")]
    NonGithubHost(String),
    #[error("could not find a git repository starting from '{0}'")]
    NoRepoContext(PathBuf),
    #[error("git config has no remote (origin or otherwise)")]
    NoRemoteConfigured,
    #[error("could not parse remote URL '{0}'")]
    InvalidRemoteUrl(String),
    #[error("git subcommand '{0}' is not supported")]
    UnknownGitSubcommand(String),
    #[error("gh subcommand '{0}' is not supported")]
    UnknownGhSubcommand(String),
    #[error("gh command is not permitted: {0}")]
    UnknownGhCommand(String),
    #[error("missing required argument for gh '{0}'")]
    MissingGhArgument(String),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Parse a GitHub remote URL in any of the supported forms.
pub fn parse_github_remote(url: &str) -> Result<GithubRemote, ResolverError> {
    if let Some(rest) = url.strip_prefix("git@") {
        return parse_scp_like(rest, url);
    }
    if let Some(rest) = url.strip_prefix("ssh://git@") {
        return parse_authority_path(rest, UrlScheme::Ssh, url);
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        return parse_authority_path(rest, UrlScheme::Ssh, url);
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return parse_authority_path(rest, UrlScheme::Https, url);
    }
    Err(ResolverError::InvalidRemoteUrl(url.to_string()))
}

fn parse_scp_like(rest: &str, original: &str) -> Result<GithubRemote, ResolverError> {
    let (host, path) = rest
        .split_once(':')
        .ok_or_else(|| ResolverError::InvalidRemoteUrl(original.to_string()))?;
    if host != GITHUB_HOST {
        return Err(ResolverError::NonGithubHost(host.to_string()));
    }
    let (org, repo) = split_org_repo(path, original)?;
    Ok(GithubRemote {
        org,
        repo,
        scheme: UrlScheme::Ssh,
    })
}

fn parse_authority_path(
    rest: &str,
    scheme: UrlScheme,
    original: &str,
) -> Result<GithubRemote, ResolverError> {
    let (host, path) = rest
        .split_once('/')
        .ok_or_else(|| ResolverError::InvalidRemoteUrl(original.to_string()))?;
    let host = host.split('@').next_back().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host != GITHUB_HOST {
        return Err(ResolverError::NonGithubHost(host.to_string()));
    }
    let (org, repo) = split_org_repo(path, original)?;
    Ok(GithubRemote { org, repo, scheme })
}

fn split_org_repo(path: &str, original: &str) -> Result<(String, String), ResolverError> {
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    let stripped = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let (org, repo) = stripped
        .split_once('/')
        .ok_or_else(|| ResolverError::InvalidRemoteUrl(original.to_string()))?;
    if org.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(ResolverError::InvalidRemoteUrl(original.to_string()));
    }
    Ok((org.to_string(), repo.to_string()))
}

fn parse_remote_url(text: &str) -> Option<String> {
    let mut current_remote: Option<String> = None;
    let mut origin_url: Option<String> = None;
    let mut first_url: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with(';') || trimmed.is_empty() {
            continue;
        }
        if let Some(name) = parse_remote_section(trimmed) {
            current_remote = Some(name);
            continue;
        }
        if trimmed.starts_with('[') {
            current_remote = None;
            continue;
        }
        if let (Some(name), Some(url)) = (current_remote.as_deref(), parse_url_assignment(trimmed))
        {
            if name == "origin" && origin_url.is_none() {
                origin_url = Some(url.to_string());
            }
            if first_url.is_none() {
                first_url = Some(url.to_string());
            }
        }
    }
    origin_url.or(first_url)
}

fn parse_remote_section(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let inner = inner.trim();
    let rest = inner.strip_prefix("remote")?;
    let rest = rest.trim_start();
    let name = rest.trim_matches('"');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn parse_url_assignment(line: &str) -> Option<&str> {
    let (key, value) = line.split_once('=')?;
    if key.trim() != "url" {
        return None;
    }
    Some(value.trim())
}

fn parse_head_branch(text: &str) -> Option<String> {
    let target = text.trim().strip_prefix("ref:")?.trim();
    target.strip_prefix("refs/heads/").map(|s| s.to_string())
}

/// A git repository discovered from a working directory, as git itself sees it.
///
/// Holds the two directories git distinguishes. The common dir owns `config`
/// and therefore the remote, shared by every worktree of the repository. The
/// private dir owns `HEAD` and therefore the branch, which differs per
/// worktree. A plain checkout has one directory in both roles; a linked
/// worktree splits them, which is why no caller is ever handed a single
/// "the git dir" to read both from.
#[derive(Debug)]
pub struct LocalRepo {
    private_dir: PathBuf,
    common_dir: PathBuf,
}

impl LocalRepo {
    /// Discover the repository containing `start` by walking upward, the way
    /// git does. The walk stops at the first `.git` entry, usable or not: an
    /// unusable one reports no repository rather than falling through and
    /// binding the request to an enclosing repository.
    pub fn discover(start: &Path) -> Option<LocalRepo> {
        let mut current = Some(start);
        while let Some(work_dir) = current {
            let git_entry = work_dir.join(".git");
            if fs::symlink_metadata(&git_entry).is_ok() {
                return LocalRepo::from_git_entry(work_dir, &git_entry);
            }
            current = work_dir.parent();
        }
        None
    }

    /// Read `remote.origin.url`, or the first remote's URL when there is no
    /// origin, from the config every worktree of this repository shares.
    pub fn origin_url(&self) -> Result<String, ResolverError> {
        let text = fs::read_to_string(self.common_dir.join("config"))?;
        parse_remote_url(&text).ok_or(ResolverError::NoRemoteConfigured)
    }

    /// Read the branch checked out in this working directory from its own
    /// `HEAD`, which a linked worktree does not share with the main checkout.
    /// `None` when HEAD is detached or unreadable.
    pub fn head_branch(&self) -> Option<String> {
        let head = fs::read_to_string(self.private_dir.join("HEAD")).ok()?;
        parse_head_branch(&head)
    }

    fn from_git_entry(work_dir: &Path, git_entry: &Path) -> Option<LocalRepo> {
        let kind = fs::metadata(git_entry).ok()?;
        let private_dir = if kind.is_dir() {
            git_entry.to_path_buf()
        } else if kind.is_file() {
            let pointer = fs::read_to_string(git_entry).ok()?;
            private_dir_from_pointer(work_dir, &pointer)?
        } else {
            return None;
        };
        let commondir = fs::read_to_string(private_dir.join("commondir")).ok();
        let common_dir = common_dir_from_pointer(&private_dir, commondir.as_deref());
        if !common_dir.join("config").is_file() {
            return None;
        }
        Some(LocalRepo {
            private_dir,
            common_dir,
        })
    }
}

/// The private git directory a `.git` file's `gitdir:` line names. A relative
/// pointer is taken from `work_dir`, the directory holding the `.git` file;
/// an absolute one stands on its own.
fn private_dir_from_pointer(work_dir: &Path, git_file_content: &str) -> Option<PathBuf> {
    let line = git_file_content.lines().next()?.trim_start();
    let target = line.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    Some(work_dir.join(target))
}

/// The common git directory a worktree's `commondir` file names. A relative
/// value is taken from the private dir; an absent file, as a submodule has,
/// makes the private dir its own common dir.
fn common_dir_from_pointer(private_dir: &Path, commondir: Option<&str>) -> PathBuf {
    match commondir.map(str::trim).filter(|target| !target.is_empty()) {
        Some(target) => private_dir.join(target),
        None => private_dir.to_path_buf(),
    }
}

/// Remote URL and HEAD branch of the repository containing `cwd`, read in the
/// invoking user's process before the request crosses the privilege boundary.
///
/// Shared by the three client-side hint passes (`ghbrk git`, `ghbrk gh`, and
/// `ghbrk explain`) so all three agree with each other, and with the
/// broker-side fallback, on which repository and worktree answered the
/// question. Either element is `None` when no repository is found or the
/// corresponding file is unreadable; neither absence is an error, since the
/// broker falls back to its own discovery when no hint is supplied.
pub fn repo_hints(cwd: &Path) -> (Option<String>, Option<String>) {
    match LocalRepo::discover(cwd) {
        Some(repo) => (repo.origin_url().ok(), repo.head_branch()),
        None => (None, None),
    }
}

/// Resolve a `git` invocation.
pub fn resolve_git(
    args: &[String],
    cwd: &Path,
    url_hint: Option<&str>,
    branch_hint: Option<&str>,
) -> Result<ResolvedRequest, ResolverError> {
    let subcmd =
        first_non_flag(args).ok_or_else(|| ResolverError::UnknownGitSubcommand(String::new()))?;
    match subcmd.as_str() {
        "push" => resolve_git_push(args, cwd, url_hint, branch_hint),
        "fetch" => resolve_git_remote_op(args, cwd, Operation::Fetch, url_hint),
        "pull" => resolve_git_remote_op(args, cwd, Operation::Pull, url_hint),
        "clone" => resolve_git_clone(args),
        other => Err(ResolverError::UnknownGitSubcommand(other.to_string())),
    }
}

/// Resolve the GitHub remote from a URL hint or by discovering the repository
/// containing `cwd`.
///
/// Returns `(remote, local_repo)`. `local_repo` is `Some` only when discovery
/// actually ran, so a caller that also needs the branch can reuse the very
/// repository the remote came from instead of walking a second time and
/// risking a different binding. A URL hint answers the remote question
/// outright and therefore suppresses discovery here; it says nothing about the
/// branch, which a caller that needs one still has to look up for itself.
fn resolve_remote_url(
    hint: Option<&str>,
    cwd: &Path,
) -> Result<(GithubRemote, Option<LocalRepo>), ResolverError> {
    match hint {
        Some(url) => {
            let remote = parse_github_remote(url)?;
            Ok((remote, None))
        }
        None => {
            let local_repo = LocalRepo::discover(cwd)
                .ok_or_else(|| ResolverError::NoRepoContext(cwd.to_path_buf()))?;
            let url = local_repo.origin_url()?;
            let remote = parse_github_remote(&url)?;
            Ok((remote, Some(local_repo)))
        }
    }
}

fn resolve_git_push(
    args: &[String],
    cwd: &Path,
    url_hint: Option<&str>,
    branch_hint: Option<&str>,
) -> Result<ResolvedRequest, ResolverError> {
    let (remote, local_repo) = resolve_remote_url(url_hint, cwd)?;
    let positional = positional_after_subcommand(args, "push");
    let refspec = positional.get(2).map(String::as_str);
    let branch = match (refspec, branch_hint) {
        (Some(spec), _) => Some(branch_from_refspec(spec, local_repo.as_ref())),
        (None, Some(hint)) => Some(hint.to_string()),
        (None, None) => local_repo.as_ref().and_then(LocalRepo::head_branch),
    };
    Ok(ResolvedRequest {
        org: remote.org,
        repo: remote.repo,
        branch,
        operation: Operation::Push,
        url_scheme: remote.scheme,
    })
}

fn resolve_git_remote_op(
    _args: &[String],
    cwd: &Path,
    operation: Operation,
    url_hint: Option<&str>,
) -> Result<ResolvedRequest, ResolverError> {
    let (remote, _local_repo) = resolve_remote_url(url_hint, cwd)?;
    Ok(ResolvedRequest {
        org: remote.org,
        repo: remote.repo,
        branch: None,
        operation,
        url_scheme: remote.scheme,
    })
}

fn resolve_git_clone(args: &[String]) -> Result<ResolvedRequest, ResolverError> {
    let positional = positional_after_subcommand(args, "clone");
    let url = positional
        .get(1)
        .ok_or_else(|| ResolverError::UnknownGitSubcommand("clone (no url)".to_string()))?;
    let remote = parse_github_remote(url)?;
    Ok(ResolvedRequest {
        org: remote.org,
        repo: remote.repo,
        branch: None,
        operation: Operation::Clone,
        url_scheme: remote.scheme,
    })
}

fn branch_from_refspec(refspec: &str, local_repo: Option<&LocalRepo>) -> String {
    if let Some((local, remote)) = refspec.split_once(':') {
        if !remote.is_empty() && remote != "HEAD" {
            return remote
                .strip_prefix("refs/heads/")
                .unwrap_or(remote)
                .to_string();
        }
        return resolve_local_side(local, local_repo);
    }
    resolve_local_side(refspec, local_repo)
}

fn resolve_local_side(local: &str, local_repo: Option<&LocalRepo>) -> String {
    if local == "HEAD" || local.is_empty() {
        if let Some(branch) = local_repo.and_then(LocalRepo::head_branch) {
            return branch;
        }
    }
    local
        .strip_prefix("refs/heads/")
        .unwrap_or(local)
        .to_string()
}

fn positional_after_subcommand(args: &[String], subcmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut found = false;
    for arg in args {
        if !found {
            if arg == subcmd {
                found = true;
                out.push(arg.clone());
            }
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

/// The first non-flag positional argument in a git invocation, skipping any
/// global flag that consumes the next argv token as its value. This is the
/// single owner of git's value-taking global-flag list; the shim (`ghbrk
/// git`) shares it to decide whether an invocation is a remote operation, so
/// both agree on which token is the subcommand.
pub fn first_non_flag(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') {
            return Some(arg.clone());
        }
        if git_global_flag_takes_value(arg) {
            iter.next();
        }
    }
    None
}

fn git_global_flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "-c" | "-C" | "--config" | "--git-dir" | "--work-tree" | "--namespace"
    )
}

/// Resolve a `gh` invocation.
pub fn resolve_gh(
    args: &[String],
    cwd: &Path,
    url_hint: Option<&str>,
    branch_hint: Option<&str>,
) -> Result<ResolvedRequest, ResolverError> {
    let operation = classify_gh(args)?;
    if let Operation::GhApiRead { .. } = operation {
        return Ok(ResolvedRequest {
            org: WILDCARD.to_string(),
            repo: WILDCARD.to_string(),
            branch: None,
            operation,
            url_scheme: UrlScheme::Https,
        });
    }
    let explicit_repo = extract_repo_flag(args);
    let (remote, local_repo) = match explicit_repo {
        Some(spec) => {
            let (org, repo) = parse_org_repo_pair(&spec)?;
            let remote = GithubRemote {
                org,
                repo,
                scheme: UrlScheme::Https,
            };
            (remote, None)
        }
        None => resolve_remote_url(url_hint, cwd)?,
    };
    let branch = if operation == Operation::PrOpen {
        branch_hint.map(|b| b.to_string()).or_else(|| {
            local_repo
                .or_else(|| LocalRepo::discover(cwd))
                .and_then(|repo| repo.head_branch())
        })
    } else {
        None
    };
    Ok(ResolvedRequest {
        org: remote.org,
        repo: remote.repo,
        branch,
        operation,
        url_scheme: remote.scheme,
    })
}

fn classify_gh(args: &[String]) -> Result<Operation, ResolverError> {
    let positional = gh_positional_args(args);
    let group = positional.first().map(|s| s.as_str()).unwrap_or_default();
    let action = positional.get(1).map(|s| s.as_str()).unwrap_or_default();
    let op = match (group, action) {
        ("pr", "create") => Operation::PrOpen,
        ("pr", "comment") => Operation::PrComment,
        ("pr", "merge") => Operation::PrMerge,
        ("pr", "close") => Operation::PrClose,
        ("pr", "review") => Operation::PrReview,
        ("issue", "create") => Operation::IssueOpen,
        ("issue", "comment") => Operation::IssueComment,
        ("issue", "close") => Operation::IssueClose,
        ("release", "create") => Operation::ReleaseCreate,
        ("release", "delete") => Operation::ReleaseDelete,
        ("release", "delete-asset") => Operation::ReleaseDeleteAsset,
        ("release", "download") => Operation::ReleaseDownload,
        ("release", "edit") => Operation::ReleaseEdit,
        ("release", "list") => Operation::ReleaseList,
        ("release", "upload") => Operation::ReleaseUpload,
        ("release", "view") => Operation::ReleaseView,
        ("api", "") => return Err(ResolverError::MissingGhArgument("api".to_string())),
        ("api", path) => {
            if let Some(method) = gh_api_method(args) {
                if !method.eq_ignore_ascii_case("GET") {
                    return Err(ResolverError::UnknownGhCommand(format!(
                        "gh api -X {method}"
                    )));
                }
            }
            Operation::GhApiRead {
                path: path.to_string(),
            }
        }
        ("", _) => return Err(ResolverError::UnknownGhSubcommand(String::new())),
        (g, "") => return Err(ResolverError::MissingGhArgument(g.to_string())),
        (g, a) => return Err(ResolverError::UnknownGhSubcommand(format!("{g} {a}"))),
    };
    Ok(op)
}

/// Value-taking flags of `gh api`. Table entries are matched by exact
/// equality with the whole token, never as a prefix, so an attached short
/// value (`-XPOST`) or an `=`-joined value (`--jq=.name`) carries its own
/// value and is not treated as an occurrence of the flag — it consumes no
/// following token. Boolean flags (`--paginate`, `--slurp`, `--silent`,
/// `--verbose`, `-i`/`--include`) are deliberately absent: listing one here
/// would make the resolver swallow the API path that follows it.
const GH_API_VALUE_FLAGS: &[&str] = &[
    "-X",
    "--method",
    "-F",
    "--field",
    "-f",
    "--raw-field",
    "-H",
    "--header",
    "-q",
    "--jq",
    "-t",
    "--template",
    "--input",
    "--cache",
    "--hostname",
];

/// Collects the positional (non-flag) arguments of a `gh` invocation.
///
/// The `gh api` arity table (skipping the value that follows a value-taking
/// flag such as `-X`/`--method`) applies only when the invocation's first
/// non-flag token is `api`. Every other invocation (`gh pr`, `gh issue`,
/// `gh release`, …) keeps the pre-existing no-arity rule — the same short
/// flag spellings mean different things there, e.g. `-f` is the boolean
/// `--fill` of `gh pr create`, not the value-taking `--raw-field` of `gh
/// api` — so applying the table everywhere would change positional
/// extraction for those invocations as a side effect of fixing `gh api`.
fn gh_positional_args(args: &[String]) -> Vec<&String> {
    let is_api = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .is_some_and(|arg| arg == "api");
    if is_api {
        gh_api_positional_args(args)
    } else {
        args.iter().filter(|arg| !arg.starts_with('-')).collect()
    }
}

fn gh_api_positional_args(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if GH_API_VALUE_FLAGS.contains(&arg.as_str()) {
            iter.next();
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        out.push(arg);
    }
    out
}

/// Extracts the HTTP method requested for a `gh api` call, if any `-X`/
/// `--method` flag is present. Recognizes spaced (`-X POST`), compact
/// (`-XPOST`), and `=`-joined (`--method=POST`) forms. Returns `None` when no
/// method flag is present (gh defaults to GET).
fn gh_api_method(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-X" || arg == "--method" {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix("--method=") {
            return Some(rest.to_string());
        }
        if let Some(rest) = arg.strip_prefix("-X") {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn extract_repo_flag(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-R" || arg == "--repo" {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix("--repo=") {
            return Some(rest.to_string());
        }
    }
    None
}

fn parse_org_repo_pair(spec: &str) -> Result<(String, String), ResolverError> {
    let (org, repo) = spec
        .split_once('/')
        .ok_or_else(|| ResolverError::InvalidRemoteUrl(spec.to_string()))?;
    if org.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(ResolverError::InvalidRemoteUrl(spec.to_string()));
    }
    Ok((org.to_string(), repo.to_string()))
}

#[cfg(test)]
mod url_tests {
    use super::*;

    #[test]
    fn scp_like_with_git_suffix() {
        let r = parse_github_remote("git@github.com:acme/web.git").unwrap();
        assert_eq!(r.org, "acme");
        assert_eq!(r.repo, "web");
        assert_eq!(r.scheme, UrlScheme::Ssh);
    }

    #[test]
    fn scp_like_without_suffix() {
        let r = parse_github_remote("git@github.com:acme/web").unwrap();
        assert_eq!(r.repo, "web");
    }

    #[test]
    fn ssh_uri() {
        let r = parse_github_remote("ssh://git@github.com/acme/web.git").unwrap();
        assert_eq!(r.scheme, UrlScheme::Ssh);
        assert_eq!(r.org, "acme");
    }

    #[test]
    fn https_uri() {
        let r = parse_github_remote("https://github.com/acme/web").unwrap();
        assert_eq!(r.scheme, UrlScheme::Https);
        assert_eq!(r.repo, "web");
    }

    #[test]
    fn https_uri_with_dot_git() {
        let r = parse_github_remote("https://github.com/acme/web.git").unwrap();
        assert_eq!(r.repo, "web");
    }

    #[test]
    fn non_github_host_rejected() {
        let err = parse_github_remote("git@gitlab.com:acme/web.git").unwrap_err();
        assert!(matches!(err, ResolverError::NonGithubHost(h) if h == "gitlab.com"));
    }

    #[test]
    fn ssh_uri_non_github_rejected() {
        let err = parse_github_remote("ssh://git@bitbucket.org/acme/web").unwrap_err();
        assert!(matches!(err, ResolverError::NonGithubHost(_)));
    }

    #[test]
    fn malformed_url_rejected() {
        let err = parse_github_remote("not a url").unwrap_err();
        assert!(matches!(err, ResolverError::InvalidRemoteUrl(_)));
    }

    #[test]
    fn missing_repo_rejected() {
        let err = parse_github_remote("git@github.com:acme").unwrap_err();
        assert!(matches!(err, ResolverError::InvalidRemoteUrl(_)));
    }

    #[test]
    fn parse_authority_path_with_port() {
        let r = parse_github_remote("https://github.com:443/acme/web").unwrap();
        assert_eq!(r.org, "acme");
        assert_eq!(r.repo, "web");
        assert_eq!(r.scheme, UrlScheme::Https);
    }

    #[test]
    fn parse_authority_path_with_userinfo_and_port() {
        let r = parse_github_remote("https://user@github.com:443/acme/web.git").unwrap();
        assert_eq!(r.org, "acme");
        assert_eq!(r.repo, "web");
        assert_eq!(r.scheme, UrlScheme::Https);
    }

    #[test]
    fn parse_authority_path_with_port_non_github_rejected() {
        let err = parse_github_remote("https://gitlab.com:443/acme/web").unwrap_err();
        assert!(matches!(err, ResolverError::NonGithubHost(h) if h == "gitlab.com"));
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn parse_origin_quoted() {
        let cfg = "[remote \"origin\"]\n\turl = git@github.com:acme/web.git\n";
        assert_eq!(
            parse_remote_url(cfg).unwrap(),
            "git@github.com:acme/web.git"
        );
    }

    #[test]
    fn parse_first_remote_when_no_origin() {
        let cfg = "[remote \"upstream\"]\n\turl = https://github.com/acme/web.git\n";
        assert_eq!(
            parse_remote_url(cfg).unwrap(),
            "https://github.com/acme/web.git"
        );
    }

    #[test]
    fn origin_preferred_over_other() {
        let cfg = "[remote \"upstream\"]\n\turl = https://github.com/acme/up.git\n\
                   [remote \"origin\"]\n\turl = https://github.com/acme/web.git\n";
        assert_eq!(
            parse_remote_url(cfg).unwrap(),
            "https://github.com/acme/web.git"
        );
    }

    #[test]
    fn comments_ignored() {
        let cfg = "# comment\n[remote \"origin\"]\n\turl = git@github.com:acme/web.git\n";
        assert!(parse_remote_url(cfg).is_some());
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn absolute_gitdir_pointer_stands_on_its_own() {
        assert_eq!(
            private_dir_from_pointer(Path::new("/co/wt"), "gitdir: /main/.git/worktrees/wt"),
            Some(PathBuf::from("/main/.git/worktrees/wt"))
        );
    }

    /// A submodule's pointer is relative and reaches upward. The `..`
    /// components stay in the path: the kernel resolves them after symlinks,
    /// which textual normalisation would not.
    #[test]
    fn relative_gitdir_pointer_joins_the_dir_holding_the_git_file() {
        assert_eq!(
            private_dir_from_pointer(
                Path::new("/super/vendor/sub"),
                "gitdir: ../../.git/modules/vendor/sub"
            ),
            Some(PathBuf::from(
                "/super/vendor/sub/../../.git/modules/vendor/sub"
            ))
        );
    }

    #[test]
    fn content_without_a_gitdir_prefix_is_rejected() {
        assert_eq!(
            private_dir_from_pointer(Path::new("/co/wt"), "/main/.git/worktrees/wt\n"),
            None
        );
    }

    #[test]
    fn empty_git_file_is_rejected() {
        assert_eq!(private_dir_from_pointer(Path::new("/co/wt"), ""), None);
    }

    #[test]
    fn gitdir_prefix_naming_no_path_is_rejected() {
        assert_eq!(
            private_dir_from_pointer(Path::new("/co/wt"), "gitdir:   \n"),
            None
        );
    }

    /// Real git writes the `.git` file with a trailing newline; an untrimmed
    /// value names a directory that does not exist.
    #[test]
    fn trailing_newline_is_trimmed_from_the_gitdir_pointer() {
        assert_eq!(
            private_dir_from_pointer(Path::new("/co/wt"), "gitdir: /main/.git/worktrees/wt\n"),
            Some(PathBuf::from("/main/.git/worktrees/wt"))
        );
    }

    #[test]
    fn trailing_whitespace_is_trimmed_from_the_gitdir_pointer() {
        assert_eq!(
            private_dir_from_pointer(Path::new("/co/wt"), "gitdir: /main/.git/worktrees/wt  \t\n"),
            Some(PathBuf::from("/main/.git/worktrees/wt"))
        );
    }

    #[test]
    fn relative_commondir_joins_the_private_dir() {
        assert_eq!(
            common_dir_from_pointer(Path::new("/main/.git/worktrees/wt"), Some("../..")),
            PathBuf::from("/main/.git/worktrees/wt/../..")
        );
    }

    #[test]
    fn absolute_commondir_stands_on_its_own() {
        assert_eq!(
            common_dir_from_pointer(Path::new("/main/.git/worktrees/wt"), Some("/main/.git")),
            PathBuf::from("/main/.git")
        );
    }

    /// Real git writes `commondir` with a trailing newline; untrimmed, the
    /// usual `../..` names a directory that does not exist.
    #[test]
    fn trailing_newline_is_trimmed_from_the_commondir_value() {
        assert_eq!(
            common_dir_from_pointer(Path::new("/main/.git/worktrees/wt"), Some("../..\n")),
            PathBuf::from("/main/.git/worktrees/wt/../..")
        );
    }

    /// A submodule has no `commondir`: its private directory holds `config`
    /// itself and is therefore its own common directory.
    #[test]
    fn absent_commondir_leaves_the_private_dir_as_the_common_dir() {
        assert_eq!(
            common_dir_from_pointer(Path::new("/super/.git/modules/vendor/sub"), None),
            PathBuf::from("/super/.git/modules/vendor/sub")
        );
    }

    #[test]
    fn empty_commondir_leaves_the_private_dir_as_the_common_dir() {
        assert_eq!(
            common_dir_from_pointer(Path::new("/super/.git/modules/vendor/sub"), Some("\n")),
            PathBuf::from("/super/.git/modules/vendor/sub")
        );
    }

    #[test]
    fn head_naming_a_branch_yields_the_branch_name() {
        assert_eq!(
            parse_head_branch("ref: refs/heads/feature/x\n"),
            Some("feature/x".to_string())
        );
    }

    #[test]
    fn detached_head_yields_no_branch() {
        assert_eq!(
            parse_head_branch("9c1f0a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f\n"),
            None
        );
    }

    #[test]
    fn head_naming_a_ref_outside_refs_heads_yields_no_branch() {
        assert_eq!(parse_head_branch("ref: refs/tags/v1\n"), None);
    }
}

#[cfg(test)]
mod argv_tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn classify_gh_pr_create() {
        let op = classify_gh(&s(&["pr", "create", "--title", "x"])).unwrap();
        assert_eq!(op, Operation::PrOpen);
    }

    #[test]
    fn classify_gh_issue_close() {
        let op = classify_gh(&s(&["issue", "close", "42"])).unwrap();
        assert_eq!(op, Operation::IssueClose);
    }

    #[test]
    fn classify_gh_release_create() {
        let op = classify_gh(&s(&["release", "create", "v1"])).unwrap();
        assert_eq!(op, Operation::ReleaseCreate);
    }

    #[test]
    fn classify_gh_release_delete() {
        let op = classify_gh(&s(&["release", "delete", "v1", "--yes"])).unwrap();
        assert_eq!(op, Operation::ReleaseDelete);
    }

    #[test]
    fn classify_gh_release_edit() {
        let op = classify_gh(&s(&["release", "edit", "v1", "--title", "v1.0.0"])).unwrap();
        assert_eq!(op, Operation::ReleaseEdit);
    }

    #[test]
    fn classify_gh_release_upload() {
        let op = classify_gh(&s(&["release", "upload", "v1", "/tmp/asset.tar.gz"])).unwrap();
        assert_eq!(op, Operation::ReleaseUpload);
    }

    #[test]
    fn classify_gh_release_delete_asset() {
        let op = classify_gh(&s(&[
            "release",
            "delete-asset",
            "v1",
            "asset.tar.gz",
            "--yes",
        ]))
        .unwrap();
        assert_eq!(op, Operation::ReleaseDeleteAsset);
    }

    #[test]
    fn classify_gh_release_list() {
        let op = classify_gh(&s(&["release", "list", "--limit", "10"])).unwrap();
        assert_eq!(op, Operation::ReleaseList);
    }

    #[test]
    fn classify_gh_release_view() {
        let op = classify_gh(&s(&["release", "view", "v1"])).unwrap();
        assert_eq!(op, Operation::ReleaseView);
    }

    #[test]
    fn classify_gh_release_download() {
        let op = classify_gh(&s(&["release", "download", "v1", "--dir", "/tmp"])).unwrap();
        assert_eq!(op, Operation::ReleaseDownload);
    }

    #[test]
    fn classify_gh_unknown_rejected() {
        let err = classify_gh(&s(&["pr", "frobnicate"])).unwrap_err();
        assert!(matches!(err, ResolverError::UnknownGhSubcommand(_)));
    }

    #[test]
    fn classify_gh_api_user() {
        let op = classify_gh(&s(&["api", "user"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "user".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_post_rejected() {
        let err = classify_gh(&s(&["api", "-X", "POST", "repos/x"])).unwrap_err();
        assert!(matches!(err, ResolverError::UnknownGhCommand(_)));
    }

    #[test]
    fn classify_gh_api_delete_rejected() {
        let err = classify_gh(&s(&["api", "--method", "DELETE", "repos/x"])).unwrap_err();
        assert!(matches!(err, ResolverError::UnknownGhCommand(_)));
    }

    #[test]
    fn classify_gh_api_explicit_get_allowed() {
        let op = classify_gh(&s(&["api", "-X", "GET", "user"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "user".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_nested_path() {
        let op = classify_gh(&s(&["api", "repos/acme/web", "--jq", ".id"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "repos/acme/web".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_missing_path() {
        let err = classify_gh(&s(&["api"])).unwrap_err();
        assert!(matches!(err, ResolverError::MissingGhArgument(_)));
    }

    #[test]
    fn classify_gh_api_input_flag_before_path() {
        let op = classify_gh(&s(&["api", "--input", "/tmp/body.json", "repos/acme/web"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "repos/acme/web".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_jq_flag_before_path() {
        let op = classify_gh(&s(&["api", "--jq", ".name", "repos/acme/web"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "repos/acme/web".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_field_flag_before_path() {
        let op = classify_gh(&s(&["api", "-F", "name=value", "repos/acme/web"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "repos/acme/web".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_boolean_flag_does_not_consume_path() {
        let op = classify_gh(&s(&["api", "--paginate", "repos/acme/web/issues"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "repos/acme/web/issues".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_attached_short_value_consumes_no_following_token() {
        let op = classify_gh(&s(&["api", "-XPOST", "repos/acme/web"])).unwrap_err();
        // -XPOST is a non-GET method, so classification is rejected — but only
        // after correctly resolving "repos/acme/web" as the path rather than
        // treating it as a second value consumed by -XPOST.
        assert!(matches!(op, ResolverError::UnknownGhCommand(ref msg) if msg.contains("POST")));
    }

    #[test]
    fn classify_gh_api_equals_joined_value_consumes_no_following_token() {
        let op = classify_gh(&s(&["api", "--jq=.name", "repos/acme/web"])).unwrap();
        assert_eq!(
            op,
            Operation::GhApiRead {
                path: "repos/acme/web".to_string()
            }
        );
    }

    #[test]
    fn classify_gh_api_value_flag_with_no_path_is_missing_argument() {
        let err = classify_gh(&s(&["api", "--input", "/tmp/body.json"])).unwrap_err();
        assert!(matches!(err, ResolverError::MissingGhArgument(_)));
    }

    #[test]
    fn classify_gh_pr_create_boolean_fill_flag_unaffected_by_api_table() {
        let op = classify_gh(&s(&[
            "pr",
            "create",
            "-f",
            "--title",
            "Add stdin forwarding",
        ]))
        .unwrap();
        assert_eq!(op, Operation::PrOpen);
    }

    #[test]
    fn classify_gh_pr_comment() {
        let op = classify_gh(&s(&["pr", "comment", "42", "--body", "hi"])).unwrap();
        assert_eq!(op, Operation::PrComment);
    }

    #[test]
    fn classify_gh_pr_review() {
        let op = classify_gh(&s(&["pr", "review", "42", "--approve"])).unwrap();
        assert_eq!(op, Operation::PrReview);
    }

    #[test]
    fn classify_gh_issue_comment() {
        let op = classify_gh(&s(&["issue", "comment", "42", "--body", "hi"])).unwrap();
        assert_eq!(op, Operation::IssueComment);
    }

    #[test]
    fn classify_gh_release_create_with_target_and_asset() {
        let op = classify_gh(&s(&[
            "release",
            "create",
            "v1.0.0",
            "--target",
            "main",
            "--title",
            "v1.0.0",
            "--generate-notes",
            "/tmp/myapp-1.0.0.tar.gz",
        ]))
        .unwrap();
        assert_eq!(op, Operation::ReleaseCreate);
    }

    #[test]
    fn extract_repo_short_flag() {
        assert_eq!(
            extract_repo_flag(&s(&["pr", "create", "-R", "other/proj"])),
            Some("other/proj".to_string())
        );
    }

    #[test]
    fn extract_repo_long_flag() {
        assert_eq!(
            extract_repo_flag(&s(&["pr", "create", "--repo", "other/proj"])),
            Some("other/proj".to_string())
        );
    }

    #[test]
    fn extract_repo_long_flag_equals() {
        assert_eq!(
            extract_repo_flag(&s(&["pr", "create", "--repo=other/proj"])),
            Some("other/proj".to_string())
        );
    }

    #[test]
    fn first_non_flag_skips_dashes() {
        assert_eq!(
            first_non_flag(&s(&["-c", "x=y", "push", "origin"])),
            Some("push".to_string())
        );
    }

    #[test]
    fn branch_from_refspec_simple() {
        assert_eq!(branch_from_refspec("feature/x", None), "feature/x");
    }

    #[test]
    fn branch_from_refspec_with_target() {
        assert_eq!(branch_from_refspec("HEAD:refs/heads/main", None), "main");
    }

    #[test]
    fn branch_from_refspec_local_to_remote_strips_refs_heads() {
        assert_eq!(
            branch_from_refspec("feature/x:refs/heads/release/v1", None),
            "release/v1"
        );
    }

    #[test]
    fn branch_from_refspec_remote_plain_branch_name() {
        assert_eq!(branch_from_refspec("feature/x:main", None), "main");
    }

    #[test]
    fn branch_from_refspec_empty_remote_falls_back_to_local() {
        assert_eq!(branch_from_refspec("feature/x:", None), "feature/x");
    }

    #[test]
    fn branch_from_refspec_head_to_head_falls_back() {
        // Both sides HEAD with no repository in hand: the local-side fallback
        // returns the literal "HEAD".
        assert_eq!(branch_from_refspec("HEAD:HEAD", None), "HEAD");
    }

    #[test]
    fn branch_from_refspec_strips_refs_heads() {
        assert_eq!(
            branch_from_refspec("refs/heads/release/v1", None),
            "release/v1"
        );
    }
}

#[cfg(test)]
mod hint_tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// Push with a URL hint and no local .git directory resolves correctly.
    #[test]
    fn resolve_git_push_with_url_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_git(
            &s(&["push", "origin", "main"]),
            tmp.path(),
            Some("git@github.com:acme/web.git"),
            None,
        );
        let resolved = result.expect("push with url hint should succeed");
        assert_eq!(resolved.org, "acme");
        assert_eq!(resolved.repo, "web");
        assert_eq!(resolved.operation, Operation::Push);
    }

    /// Fetch with a URL hint and no local .git directory resolves correctly.
    #[test]
    fn resolve_git_fetch_with_url_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_git(
            &s(&["fetch"]),
            tmp.path(),
            Some("git@github.com:acme/web.git"),
            None,
        );
        let resolved = result.expect("fetch with url hint should succeed");
        assert_eq!(resolved.org, "acme");
        assert_eq!(resolved.repo, "web");
        assert_eq!(resolved.operation, Operation::Fetch);
    }

    /// `gh pr create` with a URL hint and no local .git directory resolves to PrOpen.
    #[test]
    fn resolve_gh_pr_create_with_url_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_gh(
            &s(&["pr", "create", "--title", "x"]),
            tmp.path(),
            Some("git@github.com:acme/web.git"),
            None,
        );
        let resolved = result.expect("gh pr create with url hint should succeed");
        assert_eq!(resolved.org, "acme");
        assert_eq!(resolved.repo, "web");
        assert_eq!(resolved.operation, Operation::PrOpen);
    }

    /// `gh pr create` with both a URL hint and a branch hint uses the branch hint
    /// instead of attempting a broker-side git dir read.
    #[test]
    fn resolve_gh_pr_create_uses_branch_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_gh(
            &s(&["pr", "create", "--title", "x"]),
            tmp.path(),
            Some("git@github.com:acme/web.git"),
            Some("feature/x"),
        );
        let resolved = result.expect("gh pr create with branch hint should succeed");
        assert_eq!(resolved.org, "acme");
        assert_eq!(resolved.repo, "web");
        assert_eq!(resolved.operation, Operation::PrOpen);
        assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
    }

    /// An explicit push refspec must win over the HEAD branch hint. The shim
    /// always sets `branch_hint` to the current HEAD, but `git push origin
    /// feature/x` targets `feature/x`, not HEAD. Policy keys off the resolved
    /// branch, so the refspec must take precedence.
    #[test]
    fn resolve_git_push_explicit_refspec_beats_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_git(
            &s(&["push", "origin", "feature/x"]),
            tmp.path(),
            Some("git@github.com:acme/web.git"),
            Some("main"),
        )
        .expect("push with refspec and hint should succeed");
        assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
    }

    /// With no explicit refspec, the HEAD branch hint is used (bypassing the
    /// broker-side file read of an unreadable git dir).
    #[test]
    fn resolve_git_push_uses_hint_when_no_refspec() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_git(
            &s(&["push"]),
            tmp.path(),
            Some("git@github.com:acme/web.git"),
            Some("main"),
        )
        .expect("push with hint and no refspec should succeed");
        assert_eq!(resolved.branch.as_deref(), Some("main"));
    }

    /// Push with no hint and no .git directory returns NoRepoContext.
    #[test]
    fn resolve_git_push_no_hint_no_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = resolve_git(&s(&["push", "origin", "main"]), tmp.path(), None, None).unwrap_err();
        assert!(
            matches!(err, ResolverError::NoRepoContext(_)),
            "expected NoRepoContext, got {err:?}"
        );
    }

    /// `gh release create` with flags and a trailing asset path resolves to
    /// `ReleaseCreate` and extracts org/repo from the URL hint.
    #[test]
    fn resolve_gh_release_create_with_target_and_asset_from_cwd_remote() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_gh(
            &s(&[
                "release",
                "create",
                "v1.0.0",
                "--target",
                "main",
                "--title",
                "v1.0.0",
                "--generate-notes",
                "/tmp/myapp-1.0.0.tar.gz",
            ]),
            tmp.path(),
            Some("https://github.com/acme/myapp.git"),
            None,
        );
        let resolved = result.expect("gh release create with url hint should succeed");
        assert_eq!(resolved.org, "acme");
        assert_eq!(resolved.repo, "myapp");
        assert_eq!(resolved.operation, Operation::ReleaseCreate);
    }
}
