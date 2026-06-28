use std::fs;
use std::path::{Path, PathBuf};

use ghbrk::policy::Operation;
use ghbrk::resolver::{
    repo_hints, resolve_gh, resolve_git, ResolvedRequest, ResolverError, UrlScheme,
};
use tempfile::TempDir;

fn make_repo(remote_url: &str, head_branch: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path(), remote_url, head_branch);
    dir
}

fn init_repo(root: &Path, remote_url: &str, head_branch: &str) {
    init_git_dir(&root.join(".git"), remote_url, head_branch);
}

/// Writes the two files this resolver reads from a git directory: `config`,
/// which names the remote, and `HEAD`, which names the branch.
fn init_git_dir(git_dir: &Path, remote_url: &str, head_branch: &str) {
    fs::create_dir_all(git_dir).unwrap();
    let config = format!("[remote \"origin\"]\n\turl = {remote_url}\n");
    fs::write(git_dir.join("config"), config).unwrap();
    let head = format!("ref: refs/heads/{head_branch}\n");
    fs::write(git_dir.join("HEAD"), head).unwrap();
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Builds a linked-worktree fixture: a main checkout at `<root>/main` with its
/// own `.git/{config,HEAD}`, and a worktree checkout at `<root>/worktree`
/// whose `.git` file points at a private directory under
/// `<root>/main/.git/worktrees/wt`, which in turn points back at the main
/// checkout's `.git` via `commondir`. Returns the `TempDir` (keep it alive
/// for the duration of the test) and the worktree checkout's path.
fn make_worktree(
    remote_url: &str,
    main_head_branch: &str,
    worktree_head_branch: &str,
) -> (TempDir, PathBuf) {
    let (root, worktree_dir, _private_dir) =
        make_worktree_with_private_dir(remote_url, main_head_branch, worktree_head_branch);
    (root, worktree_dir)
}

/// Same fixture as `make_worktree`, additionally returning the worktree's
/// private directory (`<root>/main/.git/worktrees/wt`) so a test can reach in
/// and overwrite its `HEAD`, as a detached worktree checkout would have.
fn make_worktree_with_private_dir(
    remote_url: &str,
    main_head_branch: &str,
    worktree_head_branch: &str,
) -> (TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let main_dir = root.path().join("main");
    let worktree_dir = root.path().join("worktree");
    init_repo(&main_dir, remote_url, main_head_branch);
    init_worktree(&main_dir, &worktree_dir, "wt", worktree_head_branch);
    let private_dir = main_dir.join(".git").join("worktrees").join("wt");
    (root, worktree_dir, private_dir)
}

/// Builds the private-directory and `.git`-file pieces of a linked worktree
/// by hand, matching what `git worktree add` writes: `<main_dir>/.git`
/// must already exist (via `init_repo`). Writes the `.git` file and the
/// `commondir` file each with a trailing newline, matching real git.
fn init_worktree(main_dir: &Path, worktree_dir: &Path, name: &str, worktree_head_branch: &str) {
    let private_dir = main_dir.join(".git").join("worktrees").join(name);
    fs::create_dir_all(&private_dir).unwrap();
    let head = format!("ref: refs/heads/{worktree_head_branch}\n");
    fs::write(private_dir.join("HEAD"), head).unwrap();
    fs::write(private_dir.join("commondir"), "../..\n").unwrap();
    fs::create_dir_all(worktree_dir).unwrap();
    let gitdir_line = format!("gitdir: {}\n", private_dir.display());
    fs::write(worktree_dir.join(".git"), gitdir_line).unwrap();
}

/// Builds the submodule shape: a checkout whose `.git` file holds a
/// *relative* `gitdir:` pointer (with a trailing newline, matching real
/// git) at a directory that holds `config` and `HEAD` directly and has no
/// `commondir` — its private directory is also its common directory.
/// Returns the checkout path.
fn init_git_file_repo(root: &Path, remote_url: &str, head_branch: &str) -> PathBuf {
    init_git_dir(&root.join("git_dir"), remote_url, head_branch);
    let checkout = root.join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    fs::write(checkout.join(".git"), "gitdir: ../git_dir\n").unwrap();
    checkout
}

/// Builds a checkout at `<root>/checkout` whose `.git` is a plain *directory*
/// carrying a `commondir` file that holds `pointer`, beside a separate common
/// directory at `<root>/common.git`. The two directories name different
/// remotes and different branches, so a resolution that reads either file from
/// the wrong one is visible in the assertions. Returns the checkout path.
fn init_checkout_with_commondir(root: &Path, pointer: &str) -> PathBuf {
    let checkout = root.join("checkout");
    init_repo(&checkout, "git@github.com:acme/private.git", "feature/x");
    init_git_dir(
        &root.join("common.git"),
        "git@github.com:acme/common.git",
        "trunk",
    );
    fs::write(checkout.join(".git").join("commondir"), pointer).unwrap();
    checkout
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
fn resolve_gh_release_delete() {
    let dir = make_repo("https://github.com/acme/web.git", "main");
    let resolved = resolve_gh(
        &args(&["release", "delete", "v1.0.0", "--yes"]),
        dir.path(),
        None,
        None,
    )
    .expect("resolve");
    assert_eq!(resolved.operation, Operation::ReleaseDelete);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
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

/// A `.git` whose `config` parses but declares no `[remote ...]` section at
/// all must report `NoRemoteConfigured`, not fall through to `NoRepoContext`
/// or to an enclosing repository.
#[test]
fn repo_with_no_remote_reports_no_remote_configured() {
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join(".git");
    fs::create_dir_all(&git_dir).unwrap();
    fs::write(
        git_dir.join("config"),
        "[core]\n\trepositoryformatversion = 0\n",
    )
    .unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let err = resolve_git(&args(&["push"]), dir.path(), None, None).expect_err("no remote");
    assert!(matches!(err, ResolverError::NoRemoteConfigured));
}

/// The same no-remote `.git` nested inside an outer repository must still
/// report `NoRemoteConfigured` from the nested one, never bind the request to
/// the outer repository's remote.
#[test]
fn nested_repo_with_no_remote_does_not_bind_to_outer_repo() {
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    init_repo(&outer, "git@github.com:acme/outer.git", "main");
    let inner = outer.join("inner");
    let git_dir = inner.join(".git");
    fs::create_dir_all(&git_dir).unwrap();
    fs::write(
        git_dir.join("config"),
        "[core]\n\trepositoryformatversion = 0\n",
    )
    .unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let err = resolve_git(&args(&["push"]), &inner, None, None).expect_err("no remote");
    assert!(matches!(err, ResolverError::NoRemoteConfigured));
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

// --- Worktree and `.git`-file scenarios (LocalRepo support) ---

#[test]
fn resolve_git_push_from_worktree() {
    let (_root, worktree) = make_worktree("git@github.com:acme/web.git", "main", "feature/x");
    let resolved = resolve_git(&args(&["push"]), &worktree, None, None).expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
}

#[test]
fn resolve_git_fetch_from_worktree() {
    let (_root, worktree) = make_worktree("https://github.com/acme/web.git", "main", "feature/x");
    let resolved = resolve_git(&args(&["fetch"]), &worktree, None, None).expect("resolve");
    assert_eq!(resolved.operation, Operation::Fetch);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
}

#[test]
fn resolve_git_pull_from_worktree() {
    let (_root, worktree) = make_worktree("https://github.com/acme/web.git", "main", "feature/x");
    let resolved = resolve_git(&args(&["pull"]), &worktree, None, None).expect("resolve");
    assert_eq!(resolved.operation, Operation::Pull);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
}

/// A worktree whose private `HEAD` holds a bare object id (as `git worktree
/// add --detach` writes) must resolve with `branch: None`, never leak the
/// main checkout's branch.
#[test]
fn worktree_with_detached_head_resolves_no_branch() {
    let (_root, worktree, private_dir) =
        make_worktree_with_private_dir("git@github.com:acme/web.git", "main", "feature/x");
    fs::write(
        private_dir.join("HEAD"),
        "9c1f0a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f\n",
    )
    .unwrap();

    let resolved = resolve_git(&args(&["push"]), &worktree, None, None).expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert!(resolved.branch.is_none());
}

#[test]
fn resolve_gh_pr_create_from_worktree_uses_worktree_head() {
    let (_root, worktree) = make_worktree("git@github.com:acme/web.git", "main", "feature/x");
    let resolved = resolve_gh(
        &args(&["pr", "create", "--title", "foo"]),
        &worktree,
        None,
        None,
    )
    .expect("resolve");
    assert_eq!(resolved.operation, Operation::PrOpen);
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
}

/// Pins that a URL hint does not suppress the branch lookup: `resolve_gh`
/// still reads the worktree's own `HEAD` for the branch even though the
/// remote came from the hint rather than from local discovery.
#[test]
fn resolve_gh_pr_create_with_url_hint_reads_worktree_head() {
    let (_root, worktree) = make_worktree("git@github.com:acme/web.git", "main", "feature/x");
    let resolved = resolve_gh(
        &args(&["pr", "create", "--title", "foo"]),
        &worktree,
        Some("git@github.com:acme/web.git"),
        None,
    )
    .expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
    assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
}

/// The client-side hint pass (`repo_hints`, shared by `git.rs`, `gh.rs`, and
/// `explain.rs`) must read the remote URL from the common config and the
/// branch from the worktree's own `HEAD`, exactly as `resolve_git`/
/// `resolve_gh` do.
#[test]
fn repo_hints_from_worktree() {
    let (_root, worktree) = make_worktree("git@github.com:acme/web.git", "main", "feature/x");
    let (remote_url, head_branch) = repo_hints(&worktree);
    assert_eq!(remote_url, Some("git@github.com:acme/web.git".to_string()));
    assert_eq!(head_branch, Some("feature/x".to_string()));
}

#[test]
fn reject_non_github_remote_from_worktree() {
    let (_root, worktree) = make_worktree("git@gitlab.com:acme/web.git", "main", "feature/x");
    let err =
        resolve_git(&args(&["push"]), &worktree, None, None).expect_err("non-github rejected");
    assert!(matches!(err, ResolverError::NonGithubHost(host) if host == "gitlab.com"));
}

#[test]
fn resolve_from_git_file_without_commondir() {
    let root = tempfile::tempdir().unwrap();
    let checkout = init_git_file_repo(root.path(), "git@github.com:acme/sub.git", "main");
    let resolved = resolve_git(&args(&["push"]), &checkout, None, None).expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "sub");
    assert_eq!(resolved.branch.as_deref(), Some("main"));
}

/// Real git honours a `commondir` file inside a plain `.git` *directory*, not
/// only inside the private directory a `.git` file points at: it redirects
/// which `config`, and therefore which remote, the repository has (verified
/// against git 2.47.3). The branch still comes from the checkout's own `HEAD`.
#[test]
fn plain_git_dir_with_commondir_uses_the_common_config() {
    let root = tempfile::tempdir().unwrap();
    let absolute_pointer = format!("{}\n", root.path().join("common.git").display());
    let checkout = init_checkout_with_commondir(root.path(), &absolute_pointer);

    let resolved = resolve_git(&args(&["push"]), &checkout, None, None).expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "common");
    assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
}

/// A relative `commondir` pointer in a plain `.git` directory is taken from
/// that directory, not from the working directory holding it.
#[test]
fn plain_git_dir_with_relative_commondir_uses_the_common_config() {
    let root = tempfile::tempdir().unwrap();
    let checkout = init_checkout_with_commondir(root.path(), "../../common.git\n");

    let resolved = resolve_git(&args(&["push"]), &checkout, None, None).expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "common");
    assert_eq!(resolved.branch.as_deref(), Some("feature/x"));
}

/// A linked worktree nested inside another repository's directory tree must
/// resolve to its own repo and its own branch, never to the enclosing
/// repository's remote or `HEAD` (Defect 2 in the plan).
#[test]
fn worktree_nested_in_repo_does_not_bind_to_outer_repo() {
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    init_repo(&outer, "git@github.com:acme/outer.git", "main");
    let inner = outer.join("inner");
    let main_repo = root.path().join("main_repo");
    init_repo(&main_repo, "git@github.com:acme/inner.git", "main");
    init_worktree(&main_repo, &inner, "inner-wt", "feature/inner");

    let resolved = resolve_git(&args(&["push"]), &inner, None, None).expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "inner");
    assert_eq!(resolved.branch.as_deref(), Some("feature/inner"));
}

#[test]
fn dangling_gitdir_pointer_reports_no_repo() {
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    init_repo(&outer, "git@github.com:acme/outer.git", "main");
    let inner = outer.join("inner");
    fs::create_dir_all(&inner).unwrap();
    let missing = root.path().join("missing_private_dir");
    fs::write(
        inner.join(".git"),
        format!("gitdir: {}\n", missing.display()),
    )
    .unwrap();

    let err = resolve_git(&args(&["push"]), &inner, None, None).expect_err("no repo");
    assert!(matches!(err, ResolverError::NoRepoContext(_)));
}

#[test]
fn dangling_commondir_pointer_reports_no_repo() {
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    init_repo(&outer, "git@github.com:acme/outer.git", "main");
    let inner = outer.join("inner");
    fs::create_dir_all(&inner).unwrap();

    let private_dir = root.path().join("private");
    fs::create_dir_all(&private_dir).unwrap();
    fs::write(private_dir.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
    let bogus_common = root.path().join("bogus_common");
    fs::create_dir_all(&bogus_common).unwrap();
    fs::write(
        private_dir.join("commondir"),
        format!("{}\n", bogus_common.display()),
    )
    .unwrap();

    fs::write(
        inner.join(".git"),
        format!("gitdir: {}\n", private_dir.display()),
    )
    .unwrap();

    let err = resolve_git(&args(&["push"]), &inner, None, None).expect_err("no repo");
    assert!(matches!(err, ResolverError::NoRepoContext(_)));
}

/// A `.git` directory that exists but has no `config` is unusable; discovery
/// must stop there rather than falling through to the enclosing repository.
#[test]
fn discovery_stops_at_first_git_entry() {
    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    init_repo(&outer, "git@github.com:acme/outer.git", "main");
    let inner = outer.join("inner");
    fs::create_dir_all(inner.join(".git")).unwrap();

    let err = resolve_git(&args(&["push"]), &inner, None, None).expect_err("no repo");
    assert!(matches!(err, ResolverError::NoRepoContext(_)));
}

/// A `.git` entry that is neither a directory nor a regular file (a Unix
/// domain socket, here, standing in for "some other file type") must stop
/// discovery rather than falling through to the enclosing repository.
#[test]
fn git_entry_that_is_neither_dir_nor_file_reports_no_repo() {
    use std::os::unix::net::UnixListener;

    let root = tempfile::tempdir().unwrap();
    let outer = root.path().join("outer");
    init_repo(&outer, "git@github.com:acme/outer.git", "main");
    let inner = outer.join("inner");
    fs::create_dir_all(&inner).unwrap();
    let git_entry = inner.join(".git");
    UnixListener::bind(&git_entry).expect("create socket file");

    let err = resolve_git(&args(&["push"]), &inner, None, None).expect_err("no repo");
    assert!(matches!(err, ResolverError::NoRepoContext(_)));
}

/// A `.git` symlink naming a real git directory resolves exactly as its
/// target does — unchanged behaviour, both before and after `LocalRepo`.
#[test]
fn resolve_from_symlinked_git_dir() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let real_git_dir = root.path().join("real.git");
    fs::create_dir_all(&real_git_dir).unwrap();
    let config = "[remote \"origin\"]\n\turl = git@github.com:acme/web.git\n";
    fs::write(real_git_dir.join("config"), config).unwrap();
    fs::write(real_git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let checkout = root.path().join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    symlink(&real_git_dir, checkout.join(".git")).unwrap();

    let resolved = resolve_git(&args(&["fetch"]), &checkout, None, None).expect("resolve");
    assert_eq!(resolved.org, "acme");
    assert_eq!(resolved.repo, "web");
}

// --- Real-`git` guard test (format guard for the hand-built fixtures above) ---

/// Returns true when a `git` binary is reachable and runs.
fn real_git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Runs one `git` subcommand with ambient global/system configuration excluded:
/// `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` point at paths inside
/// `config_root` that do not exist, so nothing the developer's or CI runner's
/// real `~/.gitconfig` or `/etc/gitconfig` sets (`commit.gpgsign`, a
/// repo-relative `core.hooksPath`, `worktree.useRelativePaths`) can reach the
/// fixture. Panics with the captured stderr on failure, matching
/// `tests/integration/harness.rs::make_commit`.
fn run_git(git_args: &[&str], cwd: &Path, config_root: &Path) {
    let out = std::process::Command::new("git")
        .args(git_args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", config_root.join("no-such-gitconfig"))
        .env(
            "GIT_CONFIG_SYSTEM",
            config_root.join("no-such-gitconfig-system"),
        )
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {git_args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Guards the on-disk format every hand-built fixture in this file assumes: a
/// real `git worktree add` run through actual `git`, resolved through the
/// public `resolve_git` API. With ambient configuration excluded this
/// exercises git's default *absolute* `gitdir:` pointer form (the relative
/// form stays covered by `init_git_file_repo`'s hand-built submodule fixture).
#[test]
fn resolve_git_push_from_real_git_worktree() {
    if !real_git_available() {
        eprintln!("[resolver] skipping resolve_git_push_from_real_git_worktree: git --version unavailable");
        return;
    }

    let root = tempfile::tempdir().expect("tempdir");
    let main_dir = root.path().join("main");
    fs::create_dir_all(&main_dir).unwrap();
    let worktree_dir = root.path().join("worktree");

    run_git(&["init"], &main_dir, root.path());
    run_git(
        &["config", "user.email", "ghbrk-test@example.com"],
        &main_dir,
        root.path(),
    );
    run_git(
        &["config", "user.name", "ghbrk test"],
        &main_dir,
        root.path(),
    );
    run_git(
        &["config", "remote.origin.url", "git@github.com:acme/web.git"],
        &main_dir,
        root.path(),
    );
    fs::write(main_dir.join("note.txt"), "ghbrk resolver test\n").unwrap();
    run_git(&["add", "note.txt"], &main_dir, root.path());
    run_git(&["commit", "-m", "init"], &main_dir, root.path());
    run_git(
        &[
            "worktree",
            "add",
            worktree_dir.to_str().expect("utf8 path"),
            "-b",
            "feature/x",
        ],
        &main_dir,
        root.path(),
    );

    let resolved = resolve_git(&args(&["push"]), &worktree_dir, None, None).expect("resolve");
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
