use std::fs;
use std::path::{Path, PathBuf};

use ghbrk::policy::Operation;
use ghbrk::resolver::{resolve_gh, resolve_git, ResolvedRequest, ResolverError, UrlScheme};
use tempfile::TempDir;

fn make_repo(remote_url: &str, head_branch: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path(), remote_url, head_branch);
    dir
}

fn init_repo(root: &Path, remote_url: &str, head_branch: &str) {
    let git_dir = root.join(".git");
    fs::create_dir_all(&git_dir).unwrap();
    let config = format!("[remote \"origin\"]\n\turl = {remote_url}\n");
    fs::write(git_dir.join("config"), config).unwrap();
    let head = format!("ref: refs/heads/{head_branch}\n");
    fs::write(git_dir.join("HEAD"), head).unwrap();
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn resolve_git_push() {
    let dir = make_repo("git@github.com:acme/web.git", "feature/x");
    let resolved = resolve_git(
        &args(&["push", "origin", "feature/x"]),
        dir.path(),
        None,
        None,
    )
    .expect("resolve");
    assert_eq!(
        resolved,
        ResolvedRequest {
            org: "acme".into(),
            repo: "web".into(),
            branch: Some("feature/x".into()),
            operation: Operation::Push,
            url_scheme: UrlScheme::Ssh,
        }
    );
}

#[test]
fn resolve_git_push_uses_head_when_no_refspec() {
    let dir = make_repo("git@github.com:acme/web.git", "feature/x");
    let resolved = resolve_git(&args(&["push"]), dir.path(), None, None).expect("resolve");
    assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
}

#[test]
fn resolve_git_clone_explicit_url() {
    let elsewhere = tempfile::tempdir().unwrap();
    let resolved = resolve_git(
        &args(&["clone", "https://github.com/acme/web.git", "/tmp/work"]),
        elsewhere.path(),
        None,
        None,
    )
    .expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert_eq!(resolved.operation, Operation::Clone);
    assert_eq!(resolved.url_scheme, UrlScheme::Https);
    assert!(resolved.branch.is_none());
}

#[test]
fn resolve_git_fetch() {
    let dir = make_repo("https://github.com/acme/web.git", "main");
    let resolved =
        resolve_git(&args(&["fetch", "origin"]), dir.path(), None, None).expect("resolve");
    assert_eq!(resolved.operation, Operation::Fetch);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert!(resolved.branch.is_none());
}

#[test]
fn resolve_gh_pr_create_cwd() {
    let dir = make_repo("git@github.com:acme/web.git", "feature/x");
    let resolved = resolve_gh(
        &args(&["pr", "create", "--title", "foo"]),
        dir.path(),
        None,
        None,
    )
    .expect("resolve");
    assert_eq!(resolved.operation, Operation::PrOpen);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
}

#[test]
fn resolve_gh_pr_create_repo_flag() {
    let elsewhere = tempfile::tempdir().unwrap();
    let resolved = resolve_gh(
        &args(&["pr", "create", "-R", "other/proj", "--title", "bar"]),
        elsewhere.path(),
        None,
        None,
    )
    .expect("resolve");
    assert_eq!(resolved.operation, Operation::PrOpen);
    assert_eq!(resolved.org, "other");
    assert_eq!(resolved.repo, "proj");
}

#[test]
fn resolve_gh_issue_close() {
    let dir = make_repo("https://github.com/acme/web.git", "main");
    let resolved =
        resolve_gh(&args(&["issue", "close", "42"]), dir.path(), None, None).expect("resolve");
    assert_eq!(resolved.operation, Operation::IssueClose);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert!(resolved.branch.is_none());
}

#[test]
fn reject_non_github_url() {
    let dir = make_repo("git@gitlab.com:acme/web.git", "main");
    let err = resolve_git(&args(&["push", "origin", "main"]), dir.path(), None, None)
        .expect_err("non-github");
    assert!(matches!(err, ResolverError::NonGithubHost(host) if host == "gitlab.com"));
}

#[test]
fn reject_git_outside_repo() {
    let outside = tempfile::tempdir().unwrap();
    let err = resolve_git(&args(&["push"]), outside.path(), None, None).expect_err("no repo");
    assert!(matches!(err, ResolverError::NoRepoContext(_)));
}

#[test]
fn unknown_git_subcommand_denied() {
    let dir = make_repo("git@github.com:acme/web.git", "main");
    let err = resolve_git(&args(&["unknown-cmd"]), dir.path(), None, None).expect_err("unknown");
    assert!(matches!(err, ResolverError::UnknownGitSubcommand(s) if s == "unknown-cmd"));
}

#[test]
fn resolve_git_pull() {
    let dir = make_repo("https://github.com/acme/web.git", "main");
    let resolved = resolve_git(&args(&["pull"]), dir.path(), None, None).expect("resolve");
    assert_eq!(resolved.operation, Operation::Pull);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert!(resolved.branch.is_none());
}

#[test]
fn resolve_git_pull_outside_repo() {
    let outside = tempfile::tempdir().unwrap();
    let err = resolve_git(&args(&["pull"]), outside.path(), None, None).expect_err("no repo");
    assert!(matches!(err, ResolverError::NoRepoContext(_)));
}

#[test]
fn resolve_git_pull_rejects_non_github() {
    let dir = make_repo("git@gitlab.com:acme/web.git", "main");
    let err = resolve_git(&args(&["pull"]), dir.path(), None, None).expect_err("non-github");
    assert!(matches!(err, ResolverError::NonGithubHost(h) if h == "gitlab.com"));
}

#[test]
fn walks_up_to_find_git_dir() {
    let dir = make_repo("git@github.com:acme/web.git", "main");
    let nested: PathBuf = dir.path().join("subdir/deep");
    fs::create_dir_all(&nested).unwrap();
    let resolved =
        resolve_git(&args(&["fetch"]), &nested, None, None).expect("resolve from nested");
    assert_eq!(resolved.org, "acme");
}
