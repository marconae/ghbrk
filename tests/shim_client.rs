use std::path::{Path, PathBuf};

use ghbrk::protocol::{read_frame, write_frame, Request, ServerFrame, Tool};
use ghbrk::shim::{run_shim_with_io, ShimOutcome};
use tempfile::TempDir;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const NO_BROKER_EXIT: i32 = 1;
const DENIED_EXIT: i32 = 1;

fn socket_in(dir: &TempDir) -> PathBuf {
    dir.path().join("broker.sock")
}

async fn capture(tool: Tool, args: Vec<String>, cwd: PathBuf, socket_path: &Path) -> ShimOutcome {
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let code = run_shim_with_io(
        tool,
        args,
        cwd,
        socket_path,
        "/usr/bin/git",
        None,
        None,
        &mut stdout,
        &mut stderr,
    )
    .await;
    ShimOutcome {
        code,
        stdout,
        stderr,
    }
}

/// Spawn a mock broker that runs `handler` against the first accepted connection.
///
/// The returned oneshot resolves with the `Request` frame the broker received.
fn spawn_mock_broker<F, Fut>(
    socket_path: PathBuf,
    handler: F,
) -> (JoinHandle<()>, oneshot::Receiver<Request>)
where
    F: FnOnce(UnixStream, Request) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (req_tx, req_rx) = oneshot::channel();
    let listener = UnixListener::bind(&socket_path).expect("bind mock broker socket");
    let handle = tokio::spawn(async move {
        let (mut stream, _addr) = listener.accept().await.expect("accept");
        let request: Request = read_frame(&mut stream).await.expect("read request");
        let _ = req_tx.send(request.clone());
        handler(stream, request).await;
    });
    (handle, req_rx)
}

#[tokio::test]
async fn shim_relays_git_push_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);

    let (broker, _req_rx) = spawn_mock_broker(socket.clone(), |mut stream, _req| async move {
        write_frame(&mut stream, &ServerFrame::Exit { code: 42 })
            .await
            .unwrap();
    });

    let outcome = capture(
        Tool::Git,
        vec!["push".into(), "origin".into(), "main".into()],
        PathBuf::from("/work/repo"),
        &socket,
    )
    .await;

    broker.await.unwrap();
    assert_eq!(outcome.code, 42);
}

#[tokio::test]
async fn shim_streams_stdout_realtime() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);

    let (broker, _req_rx) = spawn_mock_broker(socket.clone(), |mut stream, _req| async move {
        write_frame(
            &mut stream,
            &ServerFrame::StdoutChunk {
                data: b"hello\n".to_vec(),
            },
        )
        .await
        .unwrap();
        write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
            .await
            .unwrap();
    });

    let outcome = capture(
        Tool::Git,
        vec!["status".into()],
        PathBuf::from("/repo"),
        &socket,
    )
    .await;

    broker.await.unwrap();
    assert_eq!(outcome.code, 0);
    assert_eq!(outcome.stdout, b"hello\n");
    assert!(outcome.stderr.is_empty(), "stderr: {:?}", outcome.stderr);
}

#[tokio::test]
async fn shim_streams_stderr_realtime() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);

    let (broker, _req_rx) = spawn_mock_broker(socket.clone(), |mut stream, _req| async move {
        write_frame(
            &mut stream,
            &ServerFrame::StderrChunk {
                data: b"cloning...\n".to_vec(),
            },
        )
        .await
        .unwrap();
        write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
            .await
            .unwrap();
    });

    let outcome = capture(
        Tool::Git,
        vec!["clone".into(), "url".into()],
        PathBuf::from("/work"),
        &socket,
    )
    .await;

    broker.await.unwrap();
    assert_eq!(outcome.code, 0);
    assert_eq!(outcome.stderr, b"cloning...\n");
    assert!(outcome.stdout.is_empty(), "stdout: {:?}", outcome.stdout);
}

#[tokio::test]
async fn shim_reports_denial() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);

    let (broker, _req_rx) = spawn_mock_broker(socket.clone(), |mut stream, _req| async move {
        write_frame(
            &mut stream,
            &ServerFrame::Denied {
                reason: "not allowed".into(),
            },
        )
        .await
        .unwrap();
    });

    let outcome = capture(
        Tool::Git,
        vec!["push".into()],
        PathBuf::from("/work"),
        &socket,
    )
    .await;

    broker.await.unwrap();
    assert_ne!(outcome.code, 0, "expected non-zero exit");
    assert_eq!(outcome.code, DENIED_EXIT);
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert!(
        stderr.contains("denied"),
        "expected 'denied' in stderr: {stderr}"
    );
    assert!(
        stderr.contains("not allowed"),
        "expected reason in stderr: {stderr}"
    );
    assert!(outcome.stdout.is_empty());
}

#[tokio::test]
async fn shim_reports_missing_broker() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);
    // Do NOT bind a listener.

    let outcome = capture(
        Tool::Git,
        vec!["status".into()],
        PathBuf::from("/work"),
        &socket,
    )
    .await;

    assert_ne!(outcome.code, 0, "expected non-zero exit");
    assert_eq!(outcome.code, NO_BROKER_EXIT);
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert!(
        stderr.contains("ghbrk"),
        "expected 'ghbrk' in stderr: {stderr}"
    );
    assert!(
        stderr.contains("cannot connect") || stderr.contains("broker"),
        "expected broker connect message in stderr: {stderr}"
    );
    assert!(
        stderr.contains(socket.to_str().unwrap()),
        "expected socket path in stderr: {stderr}"
    );
}

#[tokio::test]
async fn shim_sends_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);
    let cwd = PathBuf::from("/home/alice/projects/foo");

    let (broker, req_rx) = spawn_mock_broker(socket.clone(), |mut stream, _req| async move {
        write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
            .await
            .unwrap();
    });

    let outcome = capture(
        Tool::Git,
        vec!["push".into(), "origin".into(), "main".into()],
        cwd.clone(),
        &socket,
    )
    .await;

    broker.await.unwrap();
    let received = req_rx.await.expect("broker received request");
    assert_eq!(received.cwd, cwd);
    assert_eq!(received.tool, Tool::Git);
    assert_eq!(received.args, vec!["push", "origin", "main"]);
    assert_eq!(outcome.code, 0);
}

#[tokio::test]
async fn shim_preserves_chunk_order_across_stdio_streams() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);

    let (broker, _req_rx) = spawn_mock_broker(socket.clone(), |mut stream, _req| async move {
        for chunk in [b"out-1\n".as_slice(), b"out-2\n".as_slice()] {
            write_frame(
                &mut stream,
                &ServerFrame::StdoutChunk {
                    data: chunk.to_vec(),
                },
            )
            .await
            .unwrap();
        }
        write_frame(
            &mut stream,
            &ServerFrame::StderrChunk {
                data: b"err-1\n".to_vec(),
            },
        )
        .await
        .unwrap();
        write_frame(
            &mut stream,
            &ServerFrame::StdoutChunk {
                data: b"out-3\n".to_vec(),
            },
        )
        .await
        .unwrap();
        write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
            .await
            .unwrap();
    });

    let outcome = capture(
        Tool::Git,
        vec!["clone".into()],
        PathBuf::from("/work"),
        &socket,
    )
    .await;

    broker.await.unwrap();
    assert_eq!(outcome.code, 0);
    assert_eq!(outcome.stdout, b"out-1\nout-2\nout-3\n");
    assert_eq!(outcome.stderr, b"err-1\n");
}

#[test]
fn broker_eacces_silently_execs_real_binary() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener as StdUnixListener;

    // Root bypasses DAC checks so chmod 000 would not produce EACCES.
    if nix::unistd::geteuid().is_root() {
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("broker.sock");

    // Create a real Unix socket file so connect sees a socket node, then
    // chmod 000 so the connect attempt is denied with EACCES.
    let listener = StdUnixListener::bind(&socket_path).expect("bind socket");
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o000)).expect("chmod 000 socket");
    drop(listener);

    let config_path = tmp.path().join("config.yaml");
    fs::write(&config_path, "real_git: /bin/true\n").expect("write config");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ghbrk"))
        .arg("git")
        .arg("push")
        .env("GHBRK_SOCKET", &socket_path)
        .env("GHBRK_CONFIG", &config_path)
        .output()
        .expect("spawn ghbrk");

    assert_eq!(
        output.status.code(),
        Some(0),
        "expected exit 0 from /bin/true after EACCES fallthrough; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("ghbrk:"),
        "expected no ghbrk error on EACCES fallthrough, got stderr: {stderr}"
    );
}

#[test]
fn broker_missing_still_hard_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("nonexistent.sock");

    // Use an empty config file so /etc/ghbrk/config.yaml on the host
    // (if present) cannot influence the test.
    let config_path = tmp.path().join("config.yaml");
    std::fs::write(&config_path, "").expect("write empty config");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ghbrk"))
        .arg("git")
        .arg("push")
        .env("GHBRK_SOCKET", &socket_path)
        .env("GHBRK_CONFIG", &config_path)
        .output()
        .expect("spawn ghbrk");

    assert_ne!(
        output.status.code(),
        Some(0),
        "expected non-zero exit when broker socket is missing (ENOENT)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot connect to broker"),
        "expected 'cannot connect to broker' in stderr, got: {stderr}"
    );
}

#[tokio::test]
async fn shim_reports_broker_eof_before_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let socket = socket_in(&tmp);

    let (broker, _req_rx) = spawn_mock_broker(socket.clone(), |mut stream, _req| async move {
        write_frame(
            &mut stream,
            &ServerFrame::StdoutChunk {
                data: b"partial".to_vec(),
            },
        )
        .await
        .unwrap();
        // Drop the stream without sending Exit.
        drop(stream);
    });

    let outcome = capture(
        Tool::Git,
        vec!["status".into()],
        PathBuf::from("/work"),
        &socket,
    )
    .await;

    broker.await.unwrap();
    assert_ne!(
        outcome.code, 0,
        "expected non-zero exit when broker hangs up"
    );
    assert_eq!(outcome.stdout, b"partial");
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert!(
        stderr.contains("ghbrk:") && (stderr.contains("broker") || stderr.contains("protocol")),
        "expected broker-error message in stderr: {stderr}"
    );
}
