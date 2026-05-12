use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::policy::Operation;

const GITHUB_HOST: &str = "github.com";

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
pub fn resolve_git(args: &[String], cwd: &Path) -> Result<ResolvedRequest, ResolverError> {
    let subcmd =
        first_non_flag(args).ok_or_else(|| ResolverError::UnknownGitSubcommand(String::new()))?;
    match subcmd.as_str() {
        "push" => resolve_git_push(args, cwd),
        "fetch" => resolve_git_remote_op(args, cwd, Operation::Fetch),
        "clone" => resolve_git_clone(args),
        other => Err(ResolverError::UnknownGitSubcommand(other.to_string())),
    }
}

fn resolve_git_push(args: &[String], cwd: &Path) -> Result<ResolvedRequest, ResolverError> {
    let git_dir =
        find_git_dir(cwd).ok_or_else(|| ResolverError::NoRepoContext(cwd.to_path_buf()))?;
    let url = read_origin_url(&git_dir)?;
    let remote = parse_github_remote(&url)?;
    let positional = positional_after_subcommand(args, "push");
    let refspec = positional.get(2).map(String::as_str);
    let branch = match refspec {
        Some(spec) => Some(branch_from_refspec(spec, &git_dir)),
        None => read_head_branch(&git_dir),
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
) -> Result<ResolvedRequest, ResolverError> {
    let git_dir =
        find_git_dir(cwd).ok_or_else(|| ResolverError::NoRepoContext(cwd.to_path_buf()))?;
    let url = read_origin_url(&git_dir)?;
    let remote = parse_github_remote(&url)?;
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
pub fn resolve_gh(args: &[String], cwd: &Path) -> Result<ResolvedRequest, ResolverError> {
    let (operation, after) = classify_gh(args)?;
    let explicit_repo = extract_repo_flag(args);
    let (org, repo, scheme) = match explicit_repo {
        Some(spec) => {
            let (org, repo) = parse_org_repo_pair(&spec)?;
            (org, repo, UrlScheme::Https)
        }
        None => {
            let git_dir =
                find_git_dir(cwd).ok_or_else(|| ResolverError::NoRepoContext(cwd.to_path_buf()))?;
            let url = read_origin_url(&git_dir)?;
            let remote = parse_github_remote(&url)?;
            (remote.org, remote.repo, remote.scheme)
        }
    };
    let branch = if operation == Operation::PrOpen {
        find_git_dir(cwd).and_then(|gd| read_head_branch(&gd))
    } else {
        None
    };
    let _ = after;
    Ok(ResolvedRequest {
        org,
        repo,
        branch,
        operation,
        url_scheme: scheme,
    })
}

fn classify_gh(args: &[String]) -> Result<(Operation, usize), ResolverError> {
    let positional: Vec<(usize, &String)> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| !a.starts_with('-'))
        .collect();
    let group = positional
        .first()
        .map(|(_, s)| s.as_str())
        .unwrap_or_default();
    let action = positional
        .get(1)
        .map(|(_, s)| s.as_str())
        .unwrap_or_default();
    let consumed_idx = positional.get(1).map(|(i, _)| *i).unwrap_or(0);
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
        ("", _) => return Err(ResolverError::UnknownGhSubcommand(String::new())),
        (g, "") => return Err(ResolverError::MissingGhArgument(g.to_string())),
        (g, a) => return Err(ResolverError::UnknownGhSubcommand(format!("{g} {a}"))),
    };
    Ok((op, consumed_idx))
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
        let (op, _) = classify_gh(&s(&["pr", "create", "--title", "x"])).unwrap();
        assert_eq!(op, Operation::PrOpen);
    }

    #[test]
    fn classify_gh_issue_close() {
        let (op, _) = classify_gh(&s(&["issue", "close", "42"])).unwrap();
        assert_eq!(op, Operation::IssueClose);
    }

    #[test]
    fn classify_gh_release_create() {
        let (op, _) = classify_gh(&s(&["release", "create", "v1"])).unwrap();
        assert_eq!(op, Operation::ReleaseCreate);
    }

    #[test]
    fn classify_gh_unknown_rejected() {
        let err = classify_gh(&s(&["pr", "frobnicate"])).unwrap_err();
        assert!(matches!(err, ResolverError::UnknownGhSubcommand(_)));
    }

    #[test]
    fn classify_gh_pr_comment() {
        let (op, _) = classify_gh(&s(&["pr", "comment", "42", "--body", "hi"])).unwrap();
        assert_eq!(op, Operation::PrComment);
    }

    #[test]
    fn classify_gh_pr_review() {
        let (op, _) = classify_gh(&s(&["pr", "review", "42", "--approve"])).unwrap();
        assert_eq!(op, Operation::PrReview);
    }

    #[test]
    fn classify_gh_issue_comment() {
        let (op, _) = classify_gh(&s(&["issue", "comment", "42", "--body", "hi"])).unwrap();
        assert_eq!(op, Operation::IssueComment);
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
