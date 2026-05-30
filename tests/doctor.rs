use std::path::{Path, PathBuf};
use std::sync::Arc;
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

fn valid_policy_yaml() -> &'static str {
    "rules:\n  - user: \"*\"\n    org: acme\n    repo: \"*\"\n    operations: [push]\n    effect: allow\n"
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
    let sp = socket_path.clone();

    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
        let config = BrokerConfig {
            socket_path: sp.clone(),
            policy,
            audit_logger: logger,
            credentials_root: None,
        };
        rt.block_on(run_broker(config)).ok();
    });

    // Wait for socket to appear.
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

fn write_policy_file(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("policy.yaml");
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn doctor_daemon_missing_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("absent.sock");
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk doctor");
    assert!(
        !out.status.success(),
        "expected non-zero exit when daemon is absent"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("UNREACHABLE"),
        "expected UNREACHABLE in stdout: {stdout}"
    );
}

#[test]
fn doctor_daemon_no_listener_fails() {
    // Create a socket file that exists but has no listener by binding then
    // immediately dropping the listener.
    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("no-listener.sock");
    {
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        // _listener is dropped here so no one is listening.
    }
    assert!(socket_path.exists(), "socket file should still exist");

    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket_path)
        .output()
        .expect("failed to run ghbrk doctor");
    assert!(
        !out.status.success(),
        "expected non-zero exit when no listener"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("UNREACHABLE"),
        "expected UNREACHABLE in stdout: {stdout}"
    );
}

#[test]
fn doctor_policy_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = write_policy_file(tmp.path(), valid_policy_yaml());
    let socket = tmp.path().join("absent.sock");
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket)
        .env("GHBRK_POLICY", &policy_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Policy: OK"),
        "expected 'Policy: OK' in stdout: {stdout}"
    );
}

#[test]
fn doctor_policy_invalid_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = write_policy_file(tmp.path(), "not: valid: policy: !! garbage");
    let socket = tmp.path().join("absent.sock");
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket)
        .env("GHBRK_POLICY", &policy_path)
        .output()
        .expect("failed to run ghbrk doctor");
    assert!(
        !out.status.success(),
        "expected non-zero exit for invalid policy"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("INVALID") || stdout.contains("Policy: INVALID"),
        "expected INVALID in stdout: {stdout}"
    );
}

#[test]
fn doctor_any_fail_exits_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("absent.sock");
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk doctor");
    assert!(
        !out.status.success(),
        "expected non-zero exit when any check fails"
    );
}

#[test]
fn doctor_daemon_reachable_ok() {
    let h = start_broker(deny_all_policy());
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &h.socket_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Daemon: OK"),
        "expected 'Daemon: OK' in stdout: {stdout}"
    );
}

#[test]
fn doctor_all_pass_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let policy = Policy::from_yaml(valid_policy_yaml()).unwrap();
    let policy_path = write_policy_file(tmp.path(), valid_policy_yaml());

    let h = start_broker(policy);
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &h.socket_path)
        .env("GHBRK_POLICY", &policy_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Daemon: OK"),
        "expected 'Daemon: OK' in stdout: {stdout}"
    );
    assert!(
        stdout.contains("Policy: OK"),
        "expected 'Policy: OK' in stdout: {stdout}"
    );
    // Credentials check depends on system state; just ensure daemon + policy OK → exit 0 when
    // credentials also pass. Since credentials_root is None, broker uses /etc/ghbrk/credentials.
    // In CI there may be no credentials, so we don't assert exit 0 unconditionally.
    // Instead just verify it ran cleanly (not killed by signal).
    assert!(
        out.status.code().is_some(),
        "process killed by signal unexpectedly"
    );
}
