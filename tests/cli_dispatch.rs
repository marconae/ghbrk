use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

#[test]
fn help_lists_gateway_subcommands() {
    let out = Command::new(bin())
        .arg("--help")
        .output()
        .expect("failed to run ghbrk --help");
    assert!(out.status.success(), "exit: {:?}", out.status.code());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("daemon"), "stdout: {stdout}");
    assert!(stdout.contains("doctor"), "stdout: {stdout}");
    assert!(stdout.contains("explain"), "stdout: {stdout}");
    assert!(stdout.contains("policy"), "stdout: {stdout}");
    assert!(stdout.contains("git"), "stdout: {stdout}");
    assert!(stdout.contains("gh"), "stdout: {stdout}");
    assert!(
        !stdout.contains("check"),
        "stdout must not contain 'check': {stdout}"
    );
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
fn git_push_relays_to_broker() {
    // ghbrk git push is a remote operation; with a missing broker socket it
    // must fail with exit code 1 and stderr mentioning the broker.
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
fn gh_relays_to_broker() {
    // ghbrk gh relays all invocations to broker; with a missing broker it must
    // fail with exit code 1 and stderr mentioning the broker.
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
fn git_status_returns_guidance_error() {
    // Local-only git subcommands must be rejected before any broker connection.
    // This test completes without any socket timeout.
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .args(["git", "status"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk git status");
    assert!(
        !out.status.success(),
        "expected non-zero exit for local subcommand"
    );
    assert_eq!(out.status.code(), Some(2), "expected exit code 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("directly") || stderr.contains("ghbrk git only brokers"),
        "expected guidance message in stderr: {stderr}"
    );
}

#[test]
fn git_no_subcommand_returns_guidance_error() {
    // No subcommand at all is also a local-only (non-remote) case.
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .arg("git")
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk git");
    assert!(
        !out.status.success(),
        "expected non-zero exit for ghbrk git with no subcommand"
    );
    assert_eq!(out.status.code(), Some(2), "expected exit code 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("directly") || stderr.contains("ghbrk git only brokers"),
        "expected guidance message in stderr: {stderr}"
    );
}

#[test]
fn doctor_subcommand_dispatches() {
    // doctor should report daemon unreachable and exit non-zero when no broker
    // is listening.
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .arg("doctor")
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk doctor");
    assert!(
        !out.status.success(),
        "expected non-zero exit when daemon is missing"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("UNREACHABLE"),
        "expected UNREACHABLE in stdout: {stdout}"
    );
}

#[test]
fn explain_subcommand_dispatches() {
    // explain relays to broker; with a missing broker it fails with a
    // connection error (non-zero exit).
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .args(["explain", "git", "status"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk explain git status");
    assert!(
        out.status.code().is_some(),
        "process killed by signal unexpectedly"
    );
    assert!(
        !out.status.success(),
        "expected non-zero exit when broker is missing"
    );
}

#[test]
fn policy_subcommand_dispatches() {
    // policy relays to broker; with a missing broker it fails.
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .args(["policy", "acme/web"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk policy acme/web");
    assert!(
        !out.status.success(),
        "expected non-zero exit when broker is missing"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ghbrk:") || stderr.contains("broker") || stderr.contains("connect"),
        "expected broker connection error in stderr: {stderr}"
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

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let out = Command::new(bin())
        .arg("--version")
        .output()
        .expect("failed to run ghbrk --version");
    assert!(out.status.success(), "exit status: {}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ghbrk"), "stdout: {stdout}");
}

#[test]
fn help_lists_allow_subcommand() {
    let out = Command::new(bin())
        .arg("--help")
        .output()
        .expect("failed to run ghbrk --help");
    assert!(out.status.success(), "exit: {:?}", out.status.code());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // "allow" must appear as a subcommand entry, not just a word in a description.
    // Clap formats it as "  allow  <description>" at the start of a line.
    assert!(
        stdout.lines().any(|l| l.trim_start().starts_with("allow")),
        "stdout must list 'allow' subcommand: {stdout}"
    );
}

#[test]
fn allow_dispatches_with_repo_and_operands() {
    // Without a broker running, the allow subcommand must fail with exit code 1
    // and stderr mentioning the broker — proving dispatch reaches the gateway.
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .args(["allow", "acme/web", "write"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk allow acme/web write");
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
fn allow_accepts_user_flag() {
    // --user flag is accepted and the request is dispatched to the gateway.
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = missing_socket_path(&tmp);
    let out = Command::new(bin())
        .args(["allow", "acme/web", "write", "--user", "alice"])
        .env("GHBRK_SOCKET", &socket)
        .output()
        .expect("failed to run ghbrk allow acme/web write --user alice");
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
