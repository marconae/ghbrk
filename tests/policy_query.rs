use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
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
        "rules:\n  - user: \"*\"\n    org: acme\n    repo: web\n    operations: [push]\n    branches: [\"*\"]\n    effect: allow\n",
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

#[test]
fn policy_rejects_malformed_specifier() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("absent.sock");
    let out = std::process::Command::new(bin())
        .args(["policy", "notavalidspec"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk policy notavalidspec");
    assert!(
        !out.status.success(),
        "expected non-zero exit for malformed specifier"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid") || stderr.contains("format") || stderr.contains("org/repo"),
        "expected format error in stderr: {stderr}"
    );
}

#[test]
fn policy_daemon_unreachable() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("absent.sock");
    let out = std::process::Command::new(bin())
        .args(["policy", "acme/web"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk policy acme/web");
    assert!(
        !out.status.success(),
        "expected non-zero exit when broker unreachable"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ghbrk:") || stderr.contains("broker") || stderr.contains("connect"),
        "expected broker connection error in stderr: {stderr}"
    );
}

#[test]
fn policy_lists_allowed_ops() {
    let h = start_broker(allow_push_policy());
    let out = std::process::Command::new(bin())
        .args(["policy", "acme/web"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk policy acme/web");
    assert!(
        out.status.success(),
        "expected exit 0 for valid policy query: {:?} stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("push"),
        "expected 'push' in allowed ops: {stdout}"
    );
    assert!(
        stdout.contains("allowed operations:"),
        "expected 'allowed operations:' section: {stdout}"
    );
}

#[test]
fn policy_default_deny_all_forbidden() {
    let h = start_broker(deny_all_policy());
    let out = std::process::Command::new(bin())
        .args(["policy", "acme/web"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk policy acme/web");
    assert!(
        out.status.success(),
        "expected exit 0 for deny-all policy query: {:?} stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("forbidden operations"),
        "expected 'forbidden operations' section: {stdout}"
    );
    // With deny-all, allowed section should show (none).
    assert!(
        stdout.contains("(none)"),
        "expected '(none)' in allowed section for deny-all: {stdout}"
    );
}

#[test]
fn role_granted_ops_listed_as_concrete() {
    // A rule using `operations: write` (a role name) should surface concrete ops
    // like push, pr_open etc. in the allowed list — not the bare word "write".
    let policy = Policy::from_yaml(
        "rules:\n  - user: \"*\"\n    org: acme\n    repo: web\n    operations: write\n    branches: [\"*\"]\n    effect: allow\n",
    )
    .unwrap();

    let h = start_broker(policy);
    let out = std::process::Command::new(bin())
        .args(["policy", "acme/web"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk policy acme/web");

    assert!(
        out.status.success(),
        "expected exit 0: status={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // write role includes push, fetch, pull, clone, pr_open, pr_comment,
    // pr_close, pr_merge, pr_review, issue_open, issue_comment, issue_close, gh_api_read
    for op in &["push", "fetch", "pull", "pr_open", "issue_open"] {
        assert!(
            stdout.contains(op),
            "expected concrete op '{op}' in allowed list: {stdout}"
        );
    }
    assert!(
        stdout.contains("allowed operations:"),
        "expected 'allowed operations:' section: {stdout}"
    );
}

#[test]
fn ops_outside_role_listed_forbidden() {
    // Operations not in the write role (release_create) must appear in forbidden.
    let policy = Policy::from_yaml(
        "rules:\n  - user: \"*\"\n    org: acme\n    repo: web\n    operations: write\n    branches: [\"*\"]\n    effect: allow\n",
    )
    .unwrap();

    let h = start_broker(policy);
    let out = std::process::Command::new(bin())
        .args(["policy", "acme/web"])
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk policy acme/web");

    assert!(
        out.status.success(),
        "expected exit 0: status={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // release_create is in admin but not in write
    assert!(
        stdout.contains("release_create"),
        "expected 'release_create' in output: {stdout}"
    );
    assert!(
        stdout.contains("forbidden operations"),
        "expected 'forbidden operations' section: {stdout}"
    );
    // Confirm release_create appears under forbidden, not under allowed.
    // The report format is: allowed section first, then forbidden section.
    let allowed_start = stdout.find("allowed operations:").unwrap_or(0);
    let forbidden_start = stdout.find("forbidden operations").unwrap_or(usize::MAX);
    let release_pos = stdout.find("release_create").unwrap_or(usize::MAX);
    assert!(
        release_pos > forbidden_start,
        "expected 'release_create' to appear after 'forbidden operations' section\nOutput:\n{stdout}"
    );
    // Also confirm push (in write role) appears in allowed section
    let push_pos = stdout.find("push").unwrap_or(usize::MAX);
    assert!(
        push_pos < forbidden_start && push_pos > allowed_start,
        "expected 'push' to appear in allowed section\nOutput:\n{stdout}"
    );
}
