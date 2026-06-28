use ghbrk::protocol::Tool;
use ghbrk::resolver::first_non_flag;

use super::gateway::{run_gateway, socket_path_from_env};

/// A git invocation leaves the machine only for `push`, `fetch`, `clone`, and
/// `pull`. Everything else (including an empty argv) is local-only.
fn is_remote_op(args: &[String]) -> bool {
    matches!(
        first_non_flag(args).as_deref(),
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
    let (remote_url, head_branch) = ghbrk::resolver::repo_hints(&cwd);
    run_gateway(
        Tool::Git,
        args.to_vec(),
        cwd,
        &socket_path_from_env(),
        remote_url,
        head_branch,
    )
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
    fn repo_hints_absent_outside_repo() {
        let dir = tempfile::tempdir().unwrap();
        let (remote_url, head_branch) = ghbrk::resolver::repo_hints(dir.path());
        assert_eq!(remote_url, None);
        assert_eq!(head_branch, None);
    }

    #[test]
    fn repo_hints_populated_from_plain_checkout() {
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

        let (remote_url, head_branch) = ghbrk::resolver::repo_hints(dir.path());
        assert_eq!(remote_url, Some("git@github.com:test/repo.git".to_string()));
        assert_eq!(head_branch, Some("feat".to_string()));
    }
}
