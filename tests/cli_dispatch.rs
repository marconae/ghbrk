use std::os::unix::fs::symlink;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

#[test]
fn help_flag_lists_subcommands() {
    let out = Command::new(bin())
        .arg("--help")
        .output()
        .expect("failed to run ghbrk --help");
    assert!(out.status.success(), "exit: {:?}", out.status.code());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("daemon"), "stdout: {stdout}");
    assert!(stdout.contains("git"), "stdout: {stdout}");
    assert!(stdout.contains("gh"), "stdout: {stdout}");
}

#[test]
fn ghbrk_daemon_subcommand_starts_daemon() {
    // Without a real policy file the daemon should exit cleanly with a
    // non-zero status (not crash by signal). The contract here is that the
    // `daemon` subcommand is recognised and routed to the daemon code path.
    let tmp = tempfile::tempdir().expect("tempdir");
    let bogus_policy = tmp.path().join("nonexistent-policy.yaml");
    let out = Command::new(bin())
        .arg("daemon")
        .env("GHBRK_POLICY", &bogus_policy)
        .env("GHBRK_SOCKET", tmp.path().join("broker.sock"))
        .env("GHBRK_AUDIT_LOG", tmp.path().join("audit.log"))
        .output()
        .expect("failed to run ghbrk daemon");
    assert!(
        out.status.code().is_some(),
        "process killed by signal unexpectedly"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ghbrk") && (stderr.contains("policy") || stderr.contains("not")),
        "expected diagnostic on stderr, got: {stderr}"
    );
}

fn missing_socket_path(tmp: &tempfile::TempDir) -> String {
    tmp.path()
        .join("broker.sock")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn ghbrk_git_subcommand_enters_shim() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .args(["git", "push", "origin", "main"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk git push");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected non-zero exit when broker is missing"
    );
    assert_eq!(out.status.code(), Some(1), "expected exit code 1");
    assert!(
        stderr.contains("ghbrk:") && stderr.contains("broker"),
        "stderr: {stderr}"
    );
}

#[test]
fn ghbrk_gh_subcommand_enters_shim() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .args(["gh", "pr", "create"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk gh pr create");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected non-zero exit when broker is missing"
    );
    assert_eq!(out.status.code(), Some(1), "expected exit code 1");
    assert!(
        stderr.contains("ghbrk:") && stderr.contains("broker"),
        "stderr: {stderr}"
    );
}

#[test]
fn argv0_git_symlink_enters_shim() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let link = tmp.path().join("git");
    symlink(bin(), &link).expect("symlink");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(&link)
        .args(["push", "origin", "main"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run git symlink push");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected non-zero exit when broker is missing"
    );
    assert_eq!(out.status.code(), Some(1), "expected exit code 1");
    assert!(
        stderr.contains("ghbrk:") && stderr.contains("broker"),
        "stderr: {stderr}"
    );
}

#[test]
fn argv0_gh_symlink_enters_shim() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let link = tmp.path().join("gh");
    symlink(bin(), &link).expect("symlink");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(&link)
        .args(["pr", "create"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run gh symlink pr create");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected non-zero exit when broker is missing"
    );
    assert_eq!(out.status.code(), Some(1), "expected exit code 1");
    assert!(
        stderr.contains("ghbrk:") && stderr.contains("broker"),
        "stderr: {stderr}"
    );
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let out = Command::new(bin())
        .arg("unknown-cmd")
        .output()
        .expect("failed to run ghbrk unknown-cmd");
    assert!(
        !out.status.success(),
        "expected non-zero exit but got: {:?}",
        out.status.code()
    );
}
