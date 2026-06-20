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

/// Walk upward from `start` looking for a `.git/config` file. Returns the
/// `.git` directory path if found.
pub fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(".git");
        if candidate.join("config").is_file() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

/// Read `remote.origin.url` (or first remote if origin is absent) from the
/// git config at `git_dir/config`.
pub fn read_origin_url(git_dir: &Path) -> Result<String, ResolverError> {
    let config_path = git_dir.join("config");
    let text = fs::read_to_string(&config_path)?;
    parse_remote_url(&text).ok_or(ResolverError::NoRemoteConfigured)
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

/// Read the current branch name from `git_dir/HEAD`. Returns `None` if HEAD
/// is detached or missing.
pub fn read_head_branch(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let target = head.strip_prefix("ref:")?.trim();
    target.strip_prefix("refs/heads/").map(|s| s.to_string())
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

/// Resolve the GitHub remote from a URL hint or by reading the local `.git/config`.
///
/// Returns `(remote, git_dir)` where `git_dir` is `Some` when a local repo was
/// found (needed for branch resolution) and `None` when the URL came purely
/// from the hint.
fn resolve_remote_url(
    hint: Option<&str>,
    cwd: &Path,
) -> Result<(GithubRemote, Option<PathBuf>), ResolverError> {
    match hint {
        Some(url) => {
            let remote = parse_github_remote(url)?;
            Ok((remote, None))
        }
        None => {
            let git_dir =
                find_git_dir(cwd).ok_or_else(|| ResolverError::NoRepoContext(cwd.to_path_buf()))?;
            let url = read_origin_url(&git_dir)?;
            let remote = parse_github_remote(&url)?;
            Ok((remote, Some(git_dir)))
        }
    }
}

fn resolve_git_push(
    args: &[String],
    cwd: &Path,
    url_hint: Option<&str>,
    branch_hint: Option<&str>,
) -> Result<ResolvedRequest, ResolverError> {
    let (remote, git_dir) = resolve_remote_url(url_hint, cwd)?;
    let positional = positional_after_subcommand(args, "push");
    let refspec = positional.get(2).map(String::as_str);
    let branch = match (refspec, branch_hint) {
        // An explicit refspec always names the target branch. `branch_from_refspec`
        // only reads the git dir for the HEAD/empty-local-side cases; an unreadable
        // dir makes it fall back to the literal ref, which is correct.
        (Some(spec), _) => Some(branch_from_refspec(spec, git_dir.as_deref().unwrap_or(cwd))),
        // No refspec: the hint carries the current HEAD branch and avoids a
        // broker-side read of a git dir it may not be able to access.
        (None, Some(hint)) => Some(hint.to_string()),
        // No refspec and no hint: fall back to reading HEAD from the local git dir.
        (None, None) => git_dir.as_deref().and_then(read_head_branch),
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
    let (remote, _git_dir) = resolve_remote_url(url_hint, cwd)?;
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

fn branch_from_refspec(refspec: &str, git_dir: &Path) -> String {
    if let Some((local, remote)) = refspec.split_once(':') {
        if !remote.is_empty() && remote != "HEAD" {
            return remote
                .strip_prefix("refs/heads/")
                .unwrap_or(remote)
                .to_string();
        }
        return resolve_local_side(local, git_dir);
    }
    resolve_local_side(refspec, git_dir)
}

fn resolve_local_side(local: &str, git_dir: &Path) -> String {
    if local == "HEAD" || local.is_empty() {
        if let Some(branch) = read_head_branch(git_dir) {
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

fn first_non_flag(args: &[String]) -> Option<String> {
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
    let (org, repo, scheme) = match explicit_repo {
        Some(spec) => {
            let (org, repo) = parse_org_repo_pair(&spec)?;
            (org, repo, UrlScheme::Https)
        }
        None => {
            let (remote, _git_dir) = resolve_remote_url(url_hint, cwd)?;
            (remote.org, remote.repo, remote.scheme)
        }
    };
    let branch = if operation == Operation::PrOpen {
        branch_hint
            .map(|b| b.to_string())
            .or_else(|| find_git_dir(cwd).and_then(|gd| read_head_branch(&gd)))
    } else {
        None
    };
    Ok(ResolvedRequest {
        org,
        repo,
        branch,
        operation,
        url_scheme: scheme,
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

/// Collects the positional (non-flag) arguments of a `gh` invocation,
/// skipping the value that follows a value-taking flag such as `-X`/
/// `--method` so it is not mistaken for the API path.
fn gh_positional_args(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-X" || arg == "--method" {
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
        assert_eq!(
            branch_from_refspec("feature/x", Path::new("/nonexistent")),
            "feature/x"
        );
    }

    #[test]
    fn branch_from_refspec_with_target() {
        assert_eq!(
            branch_from_refspec("HEAD:refs/heads/main", Path::new("/nonexistent")),
            "main"
        );
    }

    #[test]
    fn branch_from_refspec_local_to_remote_strips_refs_heads() {
        assert_eq!(
            branch_from_refspec("feature/x:refs/heads/release/v1", Path::new("/nonexistent")),
            "release/v1"
        );
    }

    #[test]
    fn branch_from_refspec_remote_plain_branch_name() {
        assert_eq!(
            branch_from_refspec("feature/x:main", Path::new("/nonexistent")),
            "main"
        );
    }

    #[test]
    fn branch_from_refspec_empty_remote_falls_back_to_local() {
        assert_eq!(
            branch_from_refspec("feature/x:", Path::new("/nonexistent")),
            "feature/x"
        );
    }

    #[test]
    fn branch_from_refspec_head_to_head_falls_back() {
        // Both sides HEAD with no real git dir: local-side fallback returns "HEAD".
        assert_eq!(
            branch_from_refspec("HEAD:HEAD", Path::new("/nonexistent")),
            "HEAD"
        );
    }

    #[test]
    fn branch_from_refspec_strips_refs_heads() {
        assert_eq!(
            branch_from_refspec("refs/heads/release/v1", Path::new("/nonexistent")),
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
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_push_no_repo");
        let result = resolve_git(
            &s(&["push", "origin", "main"]),
            &tmp,
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
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_fetch_no_repo");
        let result = resolve_git(
            &s(&["fetch"]),
            &tmp,
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
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_gh_no_repo");
        let result = resolve_gh(
            &s(&["pr", "create", "--title", "x"]),
            &tmp,
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
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_gh_branch_hint");
        let result = resolve_gh(
            &s(&["pr", "create", "--title", "x"]),
            &tmp,
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
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_refspec_beats_hint");
        let resolved = resolve_git(
            &s(&["push", "origin", "feature/x"]),
            &tmp,
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
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_push_hint_no_refspec");
        let resolved = resolve_git(
            &s(&["push"]),
            &tmp,
            Some("git@github.com:acme/web.git"),
            Some("main"),
        )
        .expect("push with hint and no refspec should succeed");
        assert_eq!(resolved.branch.as_deref(), Some("main"));
    }

    /// Push with no hint and no .git directory returns NoRepoContext.
    #[test]
    fn resolve_git_push_no_hint_no_repo() {
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_no_hint_no_repo");
        let err = resolve_git(&s(&["push", "origin", "main"]), &tmp, None, None).unwrap_err();
        assert!(
            matches!(err, ResolverError::NoRepoContext(_)),
            "expected NoRepoContext, got {err:?}"
        );
    }

    /// `gh release create` with flags and a trailing asset path resolves to
    /// `ReleaseCreate` and extracts org/repo from the URL hint.
    #[test]
    fn resolve_gh_release_create_with_target_and_asset_from_cwd_remote() {
        let tmp = std::env::temp_dir().join("ghbrk_hint_test_release_create_asset");
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
            &tmp,
            Some("https://github.com/acme/myapp.git"),
            None,
        );
        let resolved = result.expect("gh release create with url hint should succeed");
        assert_eq!(resolved.org, "acme");
        assert_eq!(resolved.repo, "myapp");
        assert_eq!(resolved.operation, Operation::ReleaseCreate);
    }
}
