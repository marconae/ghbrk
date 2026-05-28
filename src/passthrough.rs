use std::os::unix::process::CommandExt;
use std::process;

use crate::protocol::Tool;

fn git_global_flag_takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "-c" | "-C" | "--config" | "--git-dir" | "--work-tree" | "--namespace"
    )
}

fn first_non_flag(args: &[String]) -> Option<&str> {
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

fn git_is_broker_op(args: &[String]) -> bool {
    matches!(
        first_non_flag(args),
        Some("push" | "fetch" | "clone" | "pull")
    )
}

fn gh_is_broker_op(args: &[String]) -> bool {
    let mut positional = args.iter().filter(|a| !a.starts_with('-'));
    let group = positional.next().map(String::as_str).unwrap_or("");
    let action = positional.next().map(String::as_str).unwrap_or("");
    matches!(
        (group, action),
        ("pr", "create")
            | ("pr", "comment")
            | ("pr", "merge")
            | ("pr", "close")
            | ("pr", "review")
            | ("issue", "create")
            | ("issue", "comment")
            | ("issue", "close")
            | ("release", "create")
    )
}

/// Replace the current process image with `real_path`, passing `args` as
/// argv[1..]. On `exec` failure, prints an error to stderr and exits non-zero.
pub fn exec_passthrough(real_path: &str, args: &[String]) -> ! {
    let err = std::process::Command::new(real_path).args(args).exec();
    eprintln!("ghbrk: failed to exec {real_path}: {err}");
    process::exit(1);
}

/// Returns `true` when the invocation should bypass the broker and be exec'd
/// directly against the real binary.
pub fn is_passthrough(tool: Tool, args: &[String]) -> bool {
    match tool {
        Tool::Git => !git_is_broker_op(args),
        Tool::Gh => !gh_is_broker_op(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    // --- git broker-op tests (task 2.4) ---

    #[test]
    fn git_push_is_broker_op() {
        assert!(!is_passthrough(Tool::Git, &s(&["push", "origin", "main"])));
    }

    #[test]
    fn git_fetch_is_broker_op() {
        assert!(!is_passthrough(Tool::Git, &s(&["fetch", "origin"])));
    }

    #[test]
    fn git_clone_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Git,
            &s(&["clone", "git@github.com:acme/repo.git"])
        ));
    }

    #[test]
    fn git_push_with_c_flag_prefix_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Git,
            &s(&["-c", "http.sslVerify=false", "push", "origin"])
        ));
    }

    #[test]
    fn git_fetch_with_c_flag_prefix_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Git,
            &s(&["-c", "core.autocrlf=input", "fetch"])
        ));
    }

    #[test]
    fn git_clone_with_multiple_global_flags_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Git,
            &s(&["--git-dir", "/tmp/x/.git", "-c", "k=v", "clone", "url"])
        ));
    }

    #[test]
    fn git_status_is_passthrough() {
        assert!(is_passthrough(Tool::Git, &s(&["status"])));
    }

    #[test]
    fn git_add_is_passthrough() {
        assert!(is_passthrough(Tool::Git, &s(&["add", "."])));
    }

    #[test]
    fn git_commit_is_passthrough() {
        assert!(is_passthrough(Tool::Git, &s(&["commit", "-m", "msg"])));
    }

    #[test]
    fn git_diff_is_passthrough() {
        assert!(is_passthrough(Tool::Git, &s(&["diff"])));
    }

    #[test]
    fn git_log_is_passthrough() {
        assert!(is_passthrough(Tool::Git, &s(&["log", "--oneline"])));
    }

    #[test]
    fn git_no_subcommand_is_passthrough() {
        assert!(is_passthrough(Tool::Git, &s(&[])));
    }

    #[test]
    fn git_pull_is_broker_op() {
        assert!(!is_passthrough(Tool::Git, &s(&["pull", "origin", "main"])));
    }

    #[test]
    fn git_pull_with_c_flag_prefix_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Git,
            &s(&["-c", "k=v", "pull", "origin", "main"])
        ));
    }

    // --- gh broker-op tests (task 2.5) ---

    #[test]
    fn gh_pr_create_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Gh,
            &s(&["pr", "create", "--title", "x"])
        ));
    }

    #[test]
    fn gh_pr_comment_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Gh,
            &s(&["pr", "comment", "42", "--body", "hi"])
        ));
    }

    #[test]
    fn gh_pr_merge_is_broker_op() {
        assert!(!is_passthrough(Tool::Gh, &s(&["pr", "merge", "42"])));
    }

    #[test]
    fn gh_pr_close_is_broker_op() {
        assert!(!is_passthrough(Tool::Gh, &s(&["pr", "close", "42"])));
    }

    #[test]
    fn gh_pr_review_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Gh,
            &s(&["pr", "review", "42", "--approve"])
        ));
    }

    #[test]
    fn gh_issue_create_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Gh,
            &s(&["issue", "create", "--title", "bug"])
        ));
    }

    #[test]
    fn gh_issue_comment_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Gh,
            &s(&["issue", "comment", "1", "--body", "x"])
        ));
    }

    #[test]
    fn gh_issue_close_is_broker_op() {
        assert!(!is_passthrough(Tool::Gh, &s(&["issue", "close", "1"])));
    }

    #[test]
    fn gh_release_create_is_broker_op() {
        assert!(!is_passthrough(
            Tool::Gh,
            &s(&["release", "create", "v1.0.0"])
        ));
    }

    #[test]
    fn gh_auth_status_is_passthrough() {
        assert!(is_passthrough(Tool::Gh, &s(&["auth", "status"])));
    }

    #[test]
    fn gh_repo_view_is_passthrough() {
        assert!(is_passthrough(Tool::Gh, &s(&["repo", "view"])));
    }

    #[test]
    fn gh_pr_frobnicate_is_passthrough() {
        assert!(is_passthrough(Tool::Gh, &s(&["pr", "frobnicate"])));
    }

    #[test]
    fn gh_bare_invocation_is_passthrough() {
        assert!(is_passthrough(Tool::Gh, &s(&[])));
    }
}
