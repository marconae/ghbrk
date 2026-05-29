//! Integration tests for `ghbrk check`.
//!
//! Each test sets `GHBRK_CREDENTIALS_ROOT` to a temporary directory and
//! `USER` to a fixed name so the binary looks for credentials at a
//! predictable path.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

const TEST_USER: &str = "testuser";

/// Creates a temporary credentials directory and returns the TempDir (which
/// must be kept alive for the duration of the test) along with the path to
/// the user's sub-directory.
fn setup_creds_dir() -> (TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("failed to create tempdir");
    let user_dir = tmp.path().join(TEST_USER);
    fs::create_dir_all(&user_dir).expect("failed to create user dir");
    (tmp, user_dir)
}

/// Writes `content` to `path` and sets the unix permission bits to `mode`.
fn write_file_with_mode(path: &Path, content: &str, mode: u32) {
    fs::write(path, content).expect("write file");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set permissions");
}

/// Runs `ghbrk check` with `GHBRK_CREDENTIALS_ROOT` set to `creds_root` and
/// `GH_TOKEN` removed from the environment.
fn run_check(creds_root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("check")
        .env("GHBRK_CREDENTIALS_ROOT", creds_root)
        .env("USER", TEST_USER)
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

    let out = run_check(tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("SSH key: OK"),
        "expected 'SSH key: OK', got: {stdout}"
    );
}

#[test]
fn check_ssh_key_missing() {
    let (tmp, user_dir) = setup_creds_dir();
    // id_rsa absent; only the token is present.
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);

    let out = run_check(tmp.path());
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

    let out = run_check(tmp.path());
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

    let out = run_check(tmp.path());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("Token: OK"),
        "expected 'Token: OK', got: {stdout}"
    );
}

#[test]
fn check_token_missing() {
    let (tmp, user_dir) = setup_creds_dir();
    // token absent; only the SSH key is present.
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);

    let out = run_check(tmp.path());
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

    let out = run_check(tmp.path());
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
    // Skip when no real GitHub token is available.
    let gh_token = match std::env::var("GH_TOKEN") {
        Ok(t) => t,
        Err(_) => return,
    };

    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), &gh_token, 0o600);

    let out = run_check(tmp.path());
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
    // Skip when GH_TOKEN env var is absent — this test requires network access
    // to verify that an invalid token returns 401.
    if std::env::var("GH_TOKEN").is_err() {
        return;
    }

    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    // A syntactically valid but deliberately wrong token.
    write_file_with_mode(
        &user_dir.join("token"),
        "ghp_00000000000000000000000000000000000000",
        0o600,
    );

    let out = run_check(tmp.path());
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
    // Point the GitHub ping at a port that is always refused so the transport
    // error maps to UNREACHABLE, independent of network access or GH_TOKEN.
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);

    let out = Command::new(bin())
        .arg("check")
        .env("GHBRK_CREDENTIALS_ROOT", tmp.path())
        .env("USER", TEST_USER)
        .env_remove("GH_TOKEN")
        .env("GHBRK_GITHUB_API_URL", "http://127.0.0.1:1")
        .output()
        .expect("failed to run ghbrk check");
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
    // Requires a real GitHub token to pass all three checks.
    let gh_token = match std::env::var("GH_TOKEN") {
        Ok(t) => t,
        Err(_) => return,
    };

    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("id_rsa"), "FAKE_KEY", 0o600);
    write_file_with_mode(&user_dir.join("token"), &gh_token, 0o600);

    let out = run_check(tmp.path());
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
    // A missing SSH key must cause non-zero exit regardless of other checks.
    let (tmp, user_dir) = setup_creds_dir();
    write_file_with_mode(&user_dir.join("token"), "fake-token", 0o600);
    // id_rsa is intentionally absent.

    let out = run_check(tmp.path());
    assert!(
        !out.status.success(),
        "expected non-zero exit when any check fails (SSH key missing)"
    );
}
