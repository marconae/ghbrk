//! Integration tests for `ghbrk check`.
//!
//! `ghbrk check` now routes through the broker: the broker identifies the
//! caller via SO_PEERCRED and inspects that user's credentials as the broker
//! process. Each test therefore starts an in-process broker (`MinimalDaemon`)
//! bound to a temp socket, seeds credentials under the *current* user's name
//! (the UID the broker will see over the loopback socket), and points the
//! `ghbrk check` child at the socket via `GHBRK_SOCKET`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use ghbrk::audit::AuditLogger;
use ghbrk::broker::{run_broker, username_for_uid, BrokerConfig};
use ghbrk::policy::Policy;
use tempfile::TempDir;
use tokio::runtime::Runtime;

// ── helpers ──────────────────────────────────────────────────────────────────

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

/// Serializes tests that mutate the process-global `GHBRK_GITHUB_API_URL`.
/// The broker runs in-process and reads this variable, so concurrent mutation
/// from parallel tests would race.
static API_URL_LOCK: Mutex<()> = Mutex::new(());

/// The username the broker will resolve for connections from this process.
fn current_user() -> String {
    username_for_uid(nix::unistd::Uid::current()).expect("current process must have a username")
}

/// Creates a temporary credentials root with a sub-directory for the current
/// user and returns the TempDir plus the path to that user's directory.
fn setup_creds_dir() -> (TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    // bind_listener temporarily sets umask 0o117 (process-global) while it
    // binds the broker socket; if that races with tempdir or create_dir_all,
    // newly created directories lack the execute bit and subsequent writes fail
    // with EACCES. chmod (set_permissions) is not affected by umask, so we
    // repair the permissions on both the TempDir root and the user subdir.
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700))
        .expect("set tempdir permissions");
    let user_dir = tmp.path().join(current_user());
    fs::create_dir_all(&user_dir).expect("failed to create user dir");
    fs::set_permissions(&user_dir, fs::Permissions::from_mode(0o700))
        .expect("set user_dir permissions");
    (tmp, user_dir)
}

/// Writes `content` to `path` and sets the unix permission bits to `mode`.
fn write_file_with_mode(path: &Path, content: &str, mode: u32) {
    fs::write(path, content).expect("write file");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set permissions");
}

/// An in-process broker bound to a temp socket, used to back `ghbrk check`.
struct MinimalDaemon {
    socket_path: std::path::PathBuf,
    _creds_root: TempDir,
    _socket_dir: TempDir,
    _audit_dir: TempDir,
    handle: Option<tokio::task::JoinHandle<()>>,
    _rt: Arc<Runtime>,
}

impl MinimalDaemon {
    fn new(creds_root: TempDir) -> Self {
        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("broker.sock");
        let audit_dir = tempfile::tempdir().unwrap();
        let audit_path = audit_dir.path().join("audit.log");

        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        );

        let sp = socket_path.clone();
        let cr = creds_root.path().to_path_buf();
        let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
        let config = BrokerConfig {
            socket_path: sp.clone(),
            policy: Policy::from_yaml("rules: []").unwrap(),
            audit_logger: logger,
            credentials_root: Some(cr),
        };

        let handle = rt.spawn(async move {
            let _ = run_broker(config).await;
        });

        for _ in 0..500 {
            if sp.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        MinimalDaemon {
            socket_path,
            _creds_root: creds_root,
            _socket_dir: socket_dir,
            _audit_dir: audit_dir,
            handle: Some(handle),
            _rt: rt,
        }
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for MinimalDaemon {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// Runs `ghbrk check` pointed at the daemon's socket, with `GH_TOKEN` removed.
fn run_check_with_socket(socket: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("check")
        .env("GHBRK_SOCKET", socket)
        .env_remove("GH_TOKEN")
        .output()
        .expect("failed to run ghbrk check")
}

// ── SSH key tests ─────────────────────────────────────────────────────────────

#[test]
fn check_ssh_key_ok() {
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);

    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("SSH key: OK"),
        "expected 'SSH key: OK', got: {stdout}"
    );
}

#[test]
fn check_ssh_key_missing() {
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);

    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("SSH key: MISSING"),
        "expected 'SSH key: MISSING', got: {stdout}"
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit when SSH key is missing"
    );
}

#[test]
fn check_ssh_key_bad_perms() {
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o644);
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);

    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("SSH key: BAD PERMISSIONS"),
        "expected 'SSH key: BAD PERMISSIONS', got: {stdout}"
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit when SSH key has bad permissions"
    );
}

// ── Token tests ───────────────────────────────────────────────────────────────

#[test]
fn check_token_ok() {
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);

    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("Token: OK"),
        "expected 'Token: OK', got: {stdout}"
    );
}

#[test]
fn check_token_missing() {
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);

    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("Token: MISSING"),
        "expected 'Token: MISSING', got: {stdout}"
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit when token is missing"
    );
}

#[test]
fn check_token_bad_perms() {
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o644);

    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("Token: BAD PERMISSIONS"),
        "expected 'Token: BAD PERMISSIONS', got: {stdout}"
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit when token has bad permissions"
    );
}

// ── GitHub API tests ──────────────────────────────────────────────────────────

#[test]
fn check_github_api_ok() {
    let gh_token = match std::env::var("GH_TOKEN") {
        Ok(t) => t,
        Err(_) => return,
    };

    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), &gh_token, 0o600);

    let _guard = API_URL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GHBRK_GITHUB_API_URL");
    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("GitHub API: OK (user:"),
        "expected GitHub API OK with login, got: {stdout}"
    );
    assert!(
        out.status.success(),
        "expected exit 0, got: {:?}",
        out.status.code()
    );
}

#[test]
fn check_github_api_invalid_token() {
    if std::env::var("GH_TOKEN").is_err() {
        return;
    }

    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(
        &user_dir.join("token"),
        "ghp_00000000000000000000000000000000000000",
        0o600,
    );

    let _guard = API_URL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GHBRK_GITHUB_API_URL");
    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("GitHub API: INVALID TOKEN"),
        "expected 'GitHub API: INVALID TOKEN', got: {stdout}"
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit on invalid token"
    );
}

#[test]
fn check_github_api_unreachable() {
    // The broker runs in-process and reads GHBRK_GITHUB_API_URL, so set it in
    // this process before starting the daemon. Serialize with the other
    // GitHub-API tests because the variable is process-global.
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);

    let _guard = API_URL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("GHBRK_GITHUB_API_URL", "http://127.0.0.1:1");
    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    std::env::remove_var("GHBRK_GITHUB_API_URL");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("GitHub API: UNREACHABLE"),
        "expected 'GitHub API: UNREACHABLE', got: {stdout}"
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit when GitHub API is unreachable"
    );
}

// ── Aggregation tests ─────────────────────────────────────────────────────────

#[test]
fn check_all_pass_exit_zero() {
    let gh_token = match std::env::var("GH_TOKEN") {
        Ok(t) => t,
        Err(_) => return,
    };

    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), &gh_token, 0o600);

    let _guard = API_URL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("GHBRK_GITHUB_API_URL");
    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "expected exit 0 when all checks pass, stdout: {stdout}"
    );
    assert!(stdout.contains("SSH key: OK"), "stdout: {stdout}");
    assert!(stdout.contains("Token: OK"), "stdout: {stdout}");
    assert!(stdout.contains("GitHub API: OK"), "stdout: {stdout}");
}

#[test]
fn check_any_fail_exit_nonzero() {
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);
    // id_rsa intentionally absent.

    let daemon = MinimalDaemon::new(tmp);
    let out = run_check_with_socket(daemon.socket_path());
    assert!(
        !out.status.success(),
        "expected non-zero exit when any check fails (SSH key missing)"
    );
}
