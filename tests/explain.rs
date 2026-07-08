use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use std::time::Duration;

use ghbrk::audit::AuditLogger;
use ghbrk::broker::{run_broker, BrokerConfig};
use ghbrk::policy::Policy;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

fn deny_all_policy() -> Policy {
    Policy::from_yaml("rules: []\n").unwrap()
}

fn allow_push_policy() -> Policy {
    Policy::from_yaml(
        "rules:\n  - user: \"*\"\n    org: \"*\"\n    repo: \"*\"\n    operations: [push]\n    branches: [\"*\"]\n    effect: allow\n",
    )
    .unwrap()
}

struct BrokerHandle {
    _tmp: TempDir,
    socket_path: PathBuf,
    _thread: std::thread::JoinHandle<()>,
}

fn start_broker(policy: Policy) -> BrokerHandle {
    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("broker.sock");
    let audit_path = tmp.path().join("audit.log");
    let policy_path = tmp.path().join("policy.yaml");
    let sp = socket_path.clone();

    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
        let config = BrokerConfig {
            socket_path: sp.clone(),
            policy: Arc::new(ArcSwap::from_pointee(policy)),
            policy_path,
            audit_logger: logger,
            credentials_root: None,
        };
        rt.block_on(run_broker(config)).ok();
    });

    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    BrokerHandle {
        _tmp: tmp,
        socket_path,
        _thread: thread,
    }
}

fn make_git_repo(dir: &Path, remote_url: &str, branch: &str) {
    let git_dir = dir.join(".git");
    std::fs::create_dir_all(&git_dir).unwrap();
    std::fs::write(
        git_dir.join("config"),
        format!("[remote \"origin\"]\n\turl = {remote_url}\n"),
    )
    .unwrap();
    std::fs::write(git_dir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
}

#[test]
fn explain_no_args_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("absent.sock");
    let out = std::process::Command::new(bin())
        .arg("explain")
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk explain");
    assert!(
        !out.status.success(),
        "expected non-zero exit for explain with no args"
    );
}

#[test]
fn explain_git_status_out_of_scope() {
    let h = start_broker(deny_all_policy());
    let out = std::process::Command::new(bin())
        .args(["explain", "git", "status"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk explain git status");
    assert!(
        out.status.success(),
        "explain of a local command should exit 0: {:?}",
        out.status.code()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("local") || stdout.contains("scope"),
        "expected 'local' or 'scope' in stdout: {stdout}"
    );
}

#[test]
fn explain_git_push_allow() {
    let h = start_broker(allow_push_policy());
    let repo_tmp = tempfile::tempdir().unwrap();
    make_git_repo(repo_tmp.path(), "git@github.com:acme/web.git", "main");
    let out = std::process::Command::new(bin())
        .args(["explain", "git", "push", "origin", "main"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .current_dir(repo_tmp.path())
        .output()
        .expect("failed to run ghbrk explain git push");
    assert!(
        out.status.success(),
        "explain should exit 0 for resolved case: {:?} stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("push"),
        "expected 'push' in stdout: {stdout}"
    );
    assert!(
        stdout.contains("acme/web"),
        "expected repo 'acme/web' in stdout: {stdout}"
    );
    assert!(
        stdout.contains("allow"),
        "expected 'allow' in stdout: {stdout}"
    );
    assert!(
        stdout.contains("SSH"),
        "expected SSH credential injection info in stdout: {stdout}"
    );
}

#[test]
fn explain_git_push_deny() {
    let h = start_broker(deny_all_policy());
    let repo_tmp = tempfile::tempdir().unwrap();
    make_git_repo(repo_tmp.path(), "git@github.com:acme/web.git", "main");
    let out = std::process::Command::new(bin())
        .args(["explain", "git", "push", "origin", "main"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .current_dir(repo_tmp.path())
        .output()
        .expect("failed to run ghbrk explain git push (deny)");
    assert!(
        out.status.success(),
        "explain should exit 0 even for denied operations: {:?}",
        out.status.code()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("deny"),
        "expected 'deny' in stdout: {stdout}"
    );
}

/// `gh release delete` with a granting policy resolves to `release_delete`
/// and reports an `allow` decision, proving the resolver no longer treats
/// this subcommand as unsupported now that it is classified end-to-end.
#[test]
fn explain_gh_release_delete_allow() {
    let policy = Policy::from_yaml(
        "rules:\n  - user: \"*\"\n    org: \"*\"\n    repo: \"*\"\n    operations: [release_delete]\n    branches: [\"*\"]\n    effect: allow\n",
    )
    .unwrap();
    let h = start_broker(policy);
    let out = std::process::Command::new(bin())
        .args([
            "explain", "gh", "release", "delete", "v1.0.0", "--repo", "acme/web", "--yes",
        ])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk explain gh release delete");
    assert!(
        out.status.success(),
        "explain should exit 0 for resolved case: {:?} stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("operation: release_delete"),
        "expected resolved operation in stdout: {stdout}"
    );
    assert!(
        stdout.contains("acme/web"),
        "expected repo 'acme/web' in stdout: {stdout}"
    );
    assert!(
        stdout.contains("allow"),
        "expected 'allow' in stdout: {stdout}"
    );
    assert!(
        !stdout.to_lowercase().contains("resolver error"),
        "explain must not report a resolver error for a classified release verb: {stdout}"
    );
}

/// `gh release delete` under a deny-all policy still resolves cleanly to
/// `release_delete` and reports a `deny` decision — it must never fall back
/// to a "not supported" resolver error, since that was the bug this plan
/// fixes.
#[test]
fn explain_gh_release_delete_no_resolver_error() {
    let h = start_broker(deny_all_policy());
    let out = std::process::Command::new(bin())
        .args([
            "explain", "gh", "release", "delete", "v1.0.0", "--repo", "acme/web", "--yes",
        ])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk explain gh release delete (deny)");
    assert!(
        out.status.success(),
        "explain should exit 0 even for denied operations: {:?}",
        out.status.code()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("operation: release_delete"),
        "expected resolved operation in stdout: {stdout}"
    );
    assert!(
        stdout.contains("deny"),
        "expected 'deny' in stdout: {stdout}"
    );
    assert!(
        !stdout.to_lowercase().contains("resolver error"),
        "explain must not report a resolver error for a classified release verb: {stdout}"
    );
    assert!(
        !stdout.to_lowercase().contains("not supported"),
        "explain must not report 'not supported' for a classified release verb: {stdout}"
    );
}

#[test]
fn explain_unknown_command_fails() {
    let h = start_broker(deny_all_policy());
    let out = std::process::Command::new(bin())
        .args(["explain", "notarealcmd"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk explain notarealcmd");
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown command: {:?}",
        out.status.code()
    );
}
