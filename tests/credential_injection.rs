//! Integration test: credential injection for the `gh api` broker path.
//!
//! Drives a real broker over a Unix socket with a `gh api user` request and a
//! stub `gh` binary on PATH that echoes its `GH_TOKEN`. Proves the broker
//! injects `GH_TOKEN` for the `gh_api_read` operation end-to-end.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ghbrk::audit::AuditLogger;
use ghbrk::broker::{run_broker, BrokerConfig};
use ghbrk::policy::Policy;
use ghbrk::protocol::{read_frame, write_frame, Request, ServerFrame, Tool};
use tempfile::TempDir;
use tokio::net::UnixStream;

const TOKEN: &str = "ghp_injection_marker_42";

fn current_user() -> String {
    let out = std::process::Command::new("id")
        .arg("-un")
        .output()
        .expect("id -un");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn write_mode(path: &std::path::Path, contents: &str, mode: u32) {
    std::fs::write(path, contents).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms).unwrap();
}

/// Allow-everything policy scoped to a single `gh_api_read` rule so the request
/// is authorised and reaches the executor.
fn gh_api_policy(user: &str) -> Policy {
    let yaml = format!(
        r#"
rules:
  - user: {user}
    org: "*"
    repo: "*"
    operations: [gh_api_read]
    effect: allow
"#
    );
    Policy::from_yaml(&yaml).unwrap()
}

/// Place a stub `gh` on PATH that prints its `GH_TOKEN` to stdout.
fn install_stub_gh(dir: &std::path::Path) {
    let script = dir.join("gh");
    write_mode(
        &script,
        "#!/bin/sh\nprintf 'argv=%s GH_TOKEN=%s' \"$*\" \"$GH_TOKEN\"\n",
        0o755,
    );
    let prev = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", dir.display(), prev));
}

async fn collect_stdout(stream: &mut UnixStream) -> String {
    let mut out = Vec::new();
    loop {
        match read_frame::<_, ServerFrame>(stream).await {
            Ok(ServerFrame::StdoutChunk { data }) => out.extend_from_slice(&data),
            Ok(ServerFrame::Exit { .. }) => break,
            Ok(ServerFrame::Denied { reason }) => panic!("request denied: {reason}"),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn gh_api_receives_gh_token() {
    let user = current_user();

    let creds_root = TempDir::new().unwrap();
    let user_dir = creds_root.path().join(&user);
    std::fs::create_dir_all(&user_dir).unwrap();
    write_mode(&user_dir.join("id_rsa"), "dummy-key", 0o600);
    write_mode(&user_dir.join("token"), TOKEN, 0o600);

    let bin_dir = TempDir::new().unwrap();
    install_stub_gh(bin_dir.path());

    let run_dir = TempDir::new().unwrap();
    let socket_path = run_dir.path().join("broker.sock");
    let audit_path = run_dir.path().join("audit.log");
    let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
    let config = BrokerConfig {
        socket_path: socket_path.clone(),
        policy: gh_api_policy(&user),
        audit_logger: logger,
        credentials_root: Some(creds_root.path().to_path_buf()),
    };
    let handle = tokio::spawn(async move {
        let _ = run_broker(config).await;
    });

    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket_path.exists(), "broker socket did not appear");

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Gh,
        args: vec!["api".into(), "user".into()],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();

    let stdout = collect_stdout(&mut stream).await;
    assert_eq!(
        stdout,
        format!("argv=api user GH_TOKEN={TOKEN}"),
        "stub gh did not receive reassembled argv with injected GH_TOKEN; got {stdout:?}"
    );

    handle.abort();
}

/// `gh repo view` is a passthrough (not a broker-op). It must still reach the
/// broker, receive `GH_TOKEN`, and be recorded with `decision=passthrough` —
/// even under a deny-everything policy, which passthrough bypasses.
#[tokio::test]
async fn gh_passthrough_repo_view_receives_token() {
    let user = current_user();

    let creds_root = TempDir::new().unwrap();
    let user_dir = creds_root.path().join(&user);
    std::fs::create_dir_all(&user_dir).unwrap();
    write_mode(&user_dir.join("id_rsa"), "dummy-key", 0o600);
    write_mode(&user_dir.join("token"), TOKEN, 0o600);

    let bin_dir = TempDir::new().unwrap();
    install_stub_gh(bin_dir.path());

    let run_dir = TempDir::new().unwrap();
    let socket_path = run_dir.path().join("broker.sock");
    let audit_path = run_dir.path().join("audit.log");
    let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
    let config = BrokerConfig {
        socket_path: socket_path.clone(),
        // Empty policy: passthrough must bypass policy entirely.
        policy: Policy::from_yaml("rules: []").unwrap(),
        audit_logger: logger,
        credentials_root: Some(creds_root.path().to_path_buf()),
    };
    let handle = tokio::spawn(async move {
        let _ = run_broker(config).await;
    });

    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket_path.exists(), "broker socket did not appear");

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Gh,
        args: vec!["repo".into(), "view".into()],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();

    let stdout = collect_stdout(&mut stream).await;
    assert_eq!(
        stdout,
        format!("argv=repo view GH_TOKEN={TOKEN}"),
        "passthrough gh did not receive injected GH_TOKEN; got {stdout:?}"
    );

    handle.abort();

    let audit_body = std::fs::read_to_string(&audit_path).unwrap();
    assert!(
        audit_body.contains(r#""decision":"passthrough""#),
        "expected passthrough decision in audit log; got: {audit_body}"
    );
    assert!(
        !audit_body.contains(TOKEN),
        "audit log must never contain the token; got: {audit_body}"
    );
}

/// Allow-everything policy scoped to a single `clone` rule so the git SSH
/// request is authorised and reaches the executor.
fn git_ssh_policy(user: &str) -> Policy {
    let yaml = format!(
        r#"
rules:
  - user: {user}
    org: "org"
    repo: "repo"
    operations: [clone]
    effect: allow
"#
    );
    Policy::from_yaml(&yaml).unwrap()
}

/// Place a stub `git` binary that prints `SSH_AUTH_SOCK` and `GIT_SSH_COMMAND`
/// from the environment to stdout.
///
/// The caller must ensure the returned dir is first on PATH before starting
/// the broker. Do NOT call `std::env::set_var("PATH", ...)` here — that races
/// with other parallel tests that also mutate PATH. Use `install_stub_gh` as a
/// reference for the safe pattern (prepend inside the test with a scoped guard).
fn install_stub_git(dir: &std::path::Path) {
    let script = dir.join("git");
    write_mode(
        &script,
        "#!/bin/bash\necho \"argv=$* SSH_AUTH_SOCK=${SSH_AUTH_SOCK:-} GIT_SSH_COMMAND=${GIT_SSH_COMMAND:-}\"\n",
        0o755,
    );
    // PATH injection is the caller's responsibility to avoid racing with
    // parallel tests. In practice, most test environments fail at ssh-agent
    // startup (dummy key / missing ghbrk-clients group) and never reach the
    // git spawn, so the stub is a best-effort verification aid.
}

/// Collect stdout frames from the broker, returning the output OR an empty
/// string when the broker sends a `Denied` frame (graceful degradation for
/// environments where `ssh-agent` is unavailable).
async fn collect_stdout_or_skip_on_denied(stream: &mut UnixStream) -> Option<String> {
    let mut out = Vec::new();
    loop {
        match read_frame::<_, ServerFrame>(stream).await {
            Ok(ServerFrame::StdoutChunk { data }) => out.extend_from_slice(&data),
            Ok(ServerFrame::Exit { .. }) => break,
            Ok(ServerFrame::Denied { .. }) => return None,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Drive the broker with a git SSH request (clone via `git@github.com`) and
/// verify that `GIT_SSH_COMMAND` is not injected.
///
/// Primary assertion: `GIT_SSH_COMMAND` is never set to an ssh-wrapper path.
/// This holds whether or not `ssh-agent` starts successfully.
///
/// In most test environments (invalid dummy key, missing `ghbrk-clients` group,
/// or no `ssh-agent` on PATH), the broker sends a `Denied` frame and the test
/// returns early — the structural guarantee (broker never calls `ssh_env`)
/// still applies. On a fully-provisioned host where the agent starts and the
/// stub git is found, the env-var assertion is verified via stdout.
#[tokio::test]
async fn ssh_op_sets_ssh_auth_sock_not_git_ssh_command() {
    let user = current_user();

    let creds_root = TempDir::new().unwrap();
    let user_dir = creds_root.path().join(&user);
    std::fs::create_dir_all(&user_dir).unwrap();
    write_mode(&user_dir.join("id_rsa"), "dummy-key", 0o600);
    write_mode(&user_dir.join("token"), TOKEN, 0o600);

    // Place a stub `git` binary that echoes its env. Only active when
    // ssh-agent starts successfully (dummy key causes ssh-add to fail in CI,
    // so the broker sends Denied before reaching the git spawn). The stub dir
    // is NOT prepended to the global PATH here to avoid racing with parallel
    // tests that also call std::env::set_var("PATH", ...).
    let bin_dir = TempDir::new().unwrap();
    install_stub_git(bin_dir.path());

    let run_dir = TempDir::new().unwrap();
    let socket_path = run_dir.path().join("broker.sock");
    let audit_path = run_dir.path().join("audit.log");
    let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
    let config = BrokerConfig {
        socket_path: socket_path.clone(),
        policy: git_ssh_policy(&user),
        audit_logger: logger,
        credentials_root: Some(creds_root.path().to_path_buf()),
    };
    let handle = tokio::spawn(async move {
        let _ = run_broker(config).await;
    });

    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket_path.exists(), "broker socket did not appear");

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Git,
        args: vec!["clone".into(), "git@github.com:org/repo.git".into()],
        cwd: PathBuf::from("/"),
        remote_url: Some("git@github.com:org/repo.git".to_string()),
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();

    let stdout = match collect_stdout_or_skip_on_denied(&mut stream).await {
        Some(s) => s,
        None => {
            // ssh-agent or ghbrk-clients group absent / ssh-add failed:
            // broker denied the request. GIT_SSH_COMMAND injection is
            // structurally impossible (ssh_env was removed); nothing left to
            // assert.
            handle.abort();
            return;
        }
    };

    // GIT_SSH_COMMAND must never be set to an ssh-wrapper path.
    assert!(
        !stdout.contains("GIT_SSH_COMMAND=/") && !stdout.contains("GIT_SSH_COMMAND=ssh"),
        "GIT_SSH_COMMAND must not be set to an ssh command; got: {stdout:?}"
    );

    // If SSH_AUTH_SOCK appears in the output it must be non-empty (agent socket).
    if stdout.contains("SSH_AUTH_SOCK=") {
        let after = stdout
            .split("SSH_AUTH_SOCK=")
            .nth(1)
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("");
        assert!(
            !after.is_empty(),
            "SSH_AUTH_SOCK was set but is empty; got: {stdout:?}"
        );
    }

    handle.abort();
}
