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
