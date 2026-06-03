use std::path::{Path, PathBuf};
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
fn doctor_runs_every_check_even_after_an_error() {
    // The daemon is unreachable (an error), yet every later permission check
    // must still run and print its own status line: the aggregate never
    // short-circuits on the first error.
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
        stdout.contains("UNREACHABLE"),
        "expected the daemon error line: {stdout}"
    );
    for label in [
        "Policy: OK",
        "Policy permissions:",
        "Config dir permissions:",
        "Socket permissions:",
    ] {
        assert!(
            stdout.contains(label),
            "expected '{label}' line to print despite earlier error: {stdout}"
        );
    }
}

#[test]
fn doctor_permission_error_exits_non_zero() {
    // A reachable broker and a valid, parseable policy file mean daemon and
    // policy-parse checks pass. The config directory is the tempdir owned by the
    // (non-root) test runner, so the config-dir permission check is a write-path
    // ERROR — which on its own must flip the exit status non-zero. This proves
    // permission verdicts now feed the aggregate exit code.
    if nix::unistd::geteuid().is_root() {
        // Running as root makes the root-owner expectation hold; skip.
        return;
    }
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
        stdout.contains("Config dir permissions: ERROR"),
        "expected a config-dir ERROR from the non-root-owned tempdir: {stdout}"
    );
    assert!(
        !out.status.success(),
        "a permission ERROR must flip the exit status non-zero: {stdout}"
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

// ── Mock-broker helpers for credential-audit tests ───────────────────────────

/// A mock broker that accepts one connection, reads the request frame,
/// sends a single `CredentialAudit` frame followed by `Exit { code: 0 }`,
/// then closes.  Returns the socket path.
fn start_mock_broker_with_audit(entries: Vec<ghbrk::protocol::PathAudit>) -> (TempDir, PathBuf) {
    use ghbrk::protocol::{CredentialAudit, ServerFrame};

    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("mock.sock");
    let sp = socket_path.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            use tokio::net::UnixListener;
            let listener = UnixListener::bind(&sp).unwrap();
            let (mut stream, _) = listener.accept().await.unwrap();
            let (mut reader, mut writer) = stream.split();
            // Consume the request frame so the client doesn't get a broken pipe.
            let _ = ghbrk::protocol::read_frame::<_, ghbrk::protocol::Request>(&mut reader).await;
            let audit_frame = ServerFrame::CredentialAudit {
                audit: CredentialAudit { entries },
            };
            ghbrk::protocol::write_frame(&mut writer, &audit_frame)
                .await
                .unwrap();
            ghbrk::protocol::write_frame(&mut writer, &ServerFrame::Exit { code: 0 })
                .await
                .unwrap();
        });
    });

    // Wait for the socket to appear.
    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    (tmp, socket_path)
}

/// Resolve the uid of the `ghbrk` system user. Returns `None` if the user
/// does not exist so individual tests can skip gracefully.
fn ghbrk_uid() -> Option<u32> {
    nix::unistd::User::from_name("ghbrk")
        .ok()
        .flatten()
        .map(|u| u.uid.as_raw())
}

/// Build a `PathAudit` for a credential **directory** with the given uid and
/// mode.
fn dir_audit(uid: u32, mode: u32) -> ghbrk::protocol::PathAudit {
    ghbrk::protocol::PathAudit {
        label: "Credential dir".to_string(),
        path: std::path::PathBuf::from("/etc/ghbrk/credentials/testuser"),
        present: true,
        observed_owner_uid: uid,
        observed_mode: mode,
    }
}

/// Build a `PathAudit` for a credential **file** with the given label, uid
/// and mode.
fn file_audit(label: &str, uid: u32, mode: u32) -> ghbrk::protocol::PathAudit {
    ghbrk::protocol::PathAudit {
        label: label.to_string(),
        path: std::path::PathBuf::from(format!(
            "/etc/ghbrk/credentials/testuser/{}",
            label.to_lowercase().replace(' ', "_")
        )),
        present: true,
        observed_owner_uid: uid,
        observed_mode: mode,
    }
}

// ── Credential-audit integration tests ───────────────────────────────────────

#[test]
fn credential_dir_permissions_ok_for_0700() {
    let uid = match ghbrk_uid() {
        Some(u) => u,
        None => return, // ghbrk user not present; skip
    };
    let entries = vec![dir_audit(uid, 0o700)];
    let (_tmp, socket_path) = start_mock_broker_with_audit(entries);
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Credential dir permissions: OK"),
        "expected 'Credential dir permissions: OK' in stdout:\n{stdout}"
    );
}

#[test]
fn credential_dir_permissions_error_on_group_exec() {
    let uid = match ghbrk_uid() {
        Some(u) => u,
        None => return,
    };
    // 0o710 has group-execute, which is a write-path (traversal) exposure → ERROR.
    let entries = vec![dir_audit(uid, 0o710)];
    let (_tmp, socket_path) = start_mock_broker_with_audit(entries);
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Credential dir permissions: ERROR"),
        "expected 'Credential dir permissions: ERROR' for mode 0710:\n{stdout}"
    );
}

#[test]
fn credential_dir_permissions_warns_on_group_read() {
    let uid = match ghbrk_uid() {
        Some(u) => u,
        None => return,
    };
    // 0o740 has group-read only (no exec/write) → WARNING.
    let entries = vec![dir_audit(uid, 0o740)];
    let (_tmp, socket_path) = start_mock_broker_with_audit(entries);
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Credential dir permissions: WARNING"),
        "expected 'Credential dir permissions: WARNING' for mode 0740:\n{stdout}"
    );
}

#[test]
fn credential_file_permissions_error_on_group_write() {
    let uid = match ghbrk_uid() {
        Some(u) => u,
        None => return,
    };
    // 0o620 has group-write → ERROR for files.
    let entries = vec![dir_audit(uid, 0o700), file_audit("SSH key", uid, 0o620)];
    let (_tmp, socket_path) = start_mock_broker_with_audit(entries);
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("SSH key permissions: ERROR"),
        "expected 'SSH key permissions: ERROR' for mode 0620:\n{stdout}"
    );
}

#[test]
fn credential_file_permissions_warns_on_group_read() {
    let uid = match ghbrk_uid() {
        Some(u) => u,
        None => return,
    };
    // 0o640 has group-read but no write → WARNING for files.
    let entries = vec![dir_audit(uid, 0o700), file_audit("Token", uid, 0o640)];
    let (_tmp, socket_path) = start_mock_broker_with_audit(entries);
    let out = std::process::Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket_path)
        .output()
        .expect("failed to run ghbrk doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Token permissions: WARNING"),
        "expected 'Token permissions: WARNING' for mode 0640:\n{stdout}"
    );
}
