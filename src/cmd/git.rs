use ghbrk::protocol::Tool;

use super::gateway::{run_gateway, socket_path_from_env};

/// Git global flags that consume the following argv token as their value.
fn git_global_flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "-c" | "-C" | "--config" | "--git-dir" | "--work-tree" | "--namespace"
    )
}

/// First non-flag positional argument, skipping global flags that take a value.
fn first_subcommand(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') {
            return Some(arg.as_str());
        }
        if git_global_flag_takes_value(arg) {
            iter.next();
        }
    }
    None
}

/// A git invocation leaves the machine only for `push`, `fetch`, `clone`, and
/// `pull`. Everything else (including an empty argv) is local-only.
fn is_remote_op(args: &[String]) -> bool {
    matches!(
        first_subcommand(args),
        Some("push" | "fetch" | "clone" | "pull")
    )
}

pub fn run(args: &[String]) -> ! {
    if !is_remote_op(args) {
        eprintln!(
            "error: use 'git <subcommand>' directly; ghbrk git only brokers \
             remote operations (push, fetch, clone, pull)"
        );
        std::process::exit(2);
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let (remote_url, head_branch) = resolve_hints(&cwd);
    run_gateway(
        Tool::Git,
        args.to_vec(),
        cwd,
        &socket_path_from_env(),
        remote_url,
        head_branch,
    )
}

/// Resolve the git remote URL and HEAD branch from the working directory.
///
/// Reads `.git/config` and `.git/HEAD` while running as the invoking user,
/// before the request is handed to the broker (which may run as a different
/// system user without repo read access).
fn resolve_hints(cwd: &std::path::Path) -> (Option<String>, Option<String>) {
    let git_dir = ghbrk::resolver::find_git_dir(cwd);
    let remote_url = git_dir
        .as_deref()
        .and_then(|gd| ghbrk::resolver::read_origin_url(gd).ok());
    let head_branch = git_dir
        .as_deref()
        .and_then(ghbrk::resolver::read_head_branch);
    (remote_url, head_branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn remote_subcommands_are_remote_ops() {
        assert!(is_remote_op(&s(&["push", "origin", "main"])));
        assert!(is_remote_op(&s(&["fetch", "origin"])));
        assert!(is_remote_op(&s(&["clone", "git@github.com:acme/repo.git"])));
        assert!(is_remote_op(&s(&["pull", "origin", "main"])));
    }

    #[test]
    fn remote_op_detected_behind_global_flags() {
        assert!(is_remote_op(&s(&[
            "-c",
            "http.sslVerify=false",
            "push",
            "origin"
        ])));
        assert!(is_remote_op(&s(&[
            "--git-dir",
            "/tmp/x/.git",
            "-c",
            "k=v",
            "clone",
            "url"
        ])));
    }

    #[test]
    fn local_subcommands_are_not_remote_ops() {
        assert!(!is_remote_op(&s(&["status"])));
        assert!(!is_remote_op(&s(&["add", "."])));
        assert!(!is_remote_op(&s(&["commit", "-m", "msg"])));
        assert!(!is_remote_op(&s(&["log", "--oneline"])));
    }

    #[test]
    fn empty_args_are_not_remote_ops() {
        assert!(!is_remote_op(&s(&[])));
    }

    #[test]
    fn resolve_hints_absent_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let (remote_url, head_branch) = resolve_hints(dir.path());
        assert_eq!(remote_url, None);
        assert_eq!(head_branch, None);
    }

    #[test]
    fn resolve_hints_populated_from_git_repo() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(
            git_dir.join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:test/repo.git\n",
        )
        .unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/feat\n").unwrap();

        let (remote_url, head_branch) = resolve_hints(dir.path());
        assert_eq!(remote_url, Some("git@github.com:test/repo.git".to_string()));
        assert_eq!(head_branch, Some("feat".to_string()));
    }
}
