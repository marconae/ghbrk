//! Integration tests for the broker server.
//!
//! Each test starts a broker bound to a temp Unix socket, exercises a single
//! invariant, then cleans up. Tests rely on a real Tokio runtime + real Unix
//! socket — we are testing wire behaviour, not a mock.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ghbrk::audit::AuditLogger;
use ghbrk::broker::{run_broker, BrokerConfig};
use ghbrk::policy::Policy;
use ghbrk::protocol::{read_frame, write_frame, Request, ServerFrame, Tool};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

fn dummy_policy() -> Policy {
    // Empty rule list. With default-deny semantics every request will be
    // denied — perfect for testing the broker plumbing without exercising the
    // executor.
    Policy::from_yaml("rules: []\n").unwrap()
}

struct Harness {
    _tmp: TempDir,
    socket_path: PathBuf,
    handle: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with_creds(None).await
    }

    async fn start_with_creds(credentials_root: Option<PathBuf>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let socket_path = tmp.path().join("broker.sock");
        let audit_path = tmp.path().join("audit.log");
        let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
        let config = BrokerConfig {
            socket_path: socket_path.clone(),
            policy: dummy_policy(),
            audit_logger: logger,
            credentials_root,
        };

        let handle = tokio::spawn(async move {
            let _ = run_broker(config).await;
        });

        // Wait until the socket file appears.
        for _ in 0..200 {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket_path.exists(), "broker socket did not appear");

        Self {
            _tmp: tmp,
            socket_path,
            handle,
        }
    }
}

#[tokio::test]
async fn daemon_binds_socket_with_mode_0660() {
    let h = Harness::start().await;
    let metadata = std::fs::metadata(&h.socket_path).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o660, "expected 0660, got {mode:o}");
    h.handle.abort();
}

#[tokio::test]
async fn daemon_refuses_when_socket_in_use() {
    let h = Harness::start().await;
    // Try to bind a second listener on the same path. UnixListener::bind
    // should fail with EADDRINUSE.
    let result = tokio::net::UnixListener::bind(&h.socket_path);
    assert!(
        result.is_err(),
        "second bind on same path unexpectedly succeeded"
    );
    h.handle.abort();
}

#[tokio::test]
async fn daemon_resolves_uid_via_peercred() {
    // Verify that SO_PEERCRED UID resolution works by calling peer_username
    // directly on a connected stream and checking it matches the current user.
    let h = Harness::start().await;

    // The client side of the connection; we call peer_username on the server
    // side by establishing a loopback pair via a second listener.
    let (client, server) = tokio::net::UnixStream::pair().unwrap();

    let expected_user = std::process::Command::new("id")
        .arg("-un")
        .output()
        .expect("id -un")
        .stdout;
    let expected_user = std::str::from_utf8(&expected_user)
        .unwrap()
        .trim()
        .to_string();

    // peer_username called on the server end resolves the client's UID.
    let resolved = ghbrk::broker::peer_username(&server);
    assert_eq!(
        resolved.as_deref(),
        Some(expected_user.as_str()),
        "peer_username returned {resolved:?}, expected {expected_user:?}"
    );

    drop(client);
    h.handle.abort();
}

#[tokio::test]
async fn daemon_rejects_unknown_uid() {
    // We cannot actually spawn a process under a non-existent UID without
    // privileges, so we exercise the username-resolution function directly
    // via the public API. This is the same code path the broker uses.
    use nix::unistd::Uid;
    let candidate = Uid::from_raw(0x7FFF_FFFE);
    if ghbrk::broker::username_for_uid(candidate).is_some() {
        eprintln!("Skipping: UID 0x7FFFFFFE happens to resolve on this host");
        return;
    }
    assert!(ghbrk::broker::username_for_uid(candidate).is_none());
}

#[tokio::test]
async fn daemon_handles_concurrent_connections() {
    let h = Harness::start().await;

    let mut handles = Vec::new();
    for _ in 0..10 {
        let path = h.socket_path.clone();
        handles.push(tokio::spawn(async move {
            let mut stream = UnixStream::connect(&path).await.unwrap();
            let req = Request {
                tool: Tool::Git,
                args: vec!["push".into()],
                cwd: PathBuf::from("/nonexistent/repo"),
                remote_url: None,
                head_branch: None,
            };
            write_frame(&mut stream, &req).await.unwrap();
            // Expect a Denied frame back (resolver/policy will deny because
            // there is no git repo at the cwd).
            let frame: ServerFrame = read_frame(&mut stream).await.unwrap();
            matches!(frame, ServerFrame::Denied { .. })
        }));
    }

    let mut all_denied = true;
    for h in handles {
        let denied = h.await.expect("task panicked");
        all_denied &= denied;
    }
    assert!(all_denied, "expected every concurrent client to get Denied");
    assert!(!h.handle.is_finished(), "broker exited unexpectedly");
    h.handle.abort();
}

#[tokio::test]
async fn daemon_survives_malformed_frame() {
    let h = Harness::start().await;

    // First client sends garbage.
    {
        let mut bad = UnixStream::connect(&h.socket_path).await.unwrap();
        bad.write_all(&[0xff, 0xff, 0xff, 0xff]).await.unwrap();
        bad.shutdown().await.unwrap();
    }

    // Second client should still be served.
    let mut good = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Git,
        args: vec!["push".into()],
        cwd: PathBuf::from("/nonexistent/repo"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut good, &req).await.unwrap();
    let frame: ServerFrame = read_frame(&mut good).await.unwrap();
    assert!(
        matches!(frame, ServerFrame::Denied { .. }),
        "expected Denied, got {frame:?}"
    );

    assert!(!h.handle.is_finished(), "broker died after malformed frame");
    h.handle.abort();
}

#[test]
fn socket_group_failure_logs_error() {
    use std::sync::Mutex;

    use nix::unistd::Gid;
    use tracing::subscriber::set_default;
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = BufWriter(buf.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .finish();

    let nonexistent = PathBuf::from("/nonexistent/ghbrk-test/does-not-exist.sock");
    let guard = set_default(subscriber);
    ghbrk::broker::chown_socket_to_client_group(&nonexistent, Gid::from_raw(0));
    drop(guard);

    let output = String::from_utf8(buf.lock().unwrap().clone()).expect("utf8 log output");
    assert!(
        output.contains("ERROR"),
        "expected ERROR level log, got:\n{output}"
    );
    assert!(
        output.contains("Group=ghbrk-clients"),
        "expected message to name Group=ghbrk-clients, got:\n{output}"
    );
}

#[test]
fn daemon_shuts_down_on_sigterm() {
    // Spawn the ghbrk binary as a real daemon, send it SIGTERM, and verify
    // it removes the socket file and exits with code zero. We use a
    // synchronous test (not tokio::test) because we are managing a child
    // process via std and signals, not async I/O.
    let bin = env!("CARGO_BIN_EXE_ghbrk");
    let tmp = tempfile::tempdir().unwrap();
    let socket = tmp.path().join("broker.sock");
    let audit = tmp.path().join("audit.log");
    let policy = tmp.path().join("policy.yaml");
    std::fs::write(&policy, "rules: []\n").unwrap();

    let mut child = std::process::Command::new(bin)
        .arg("daemon")
        .env("GHBRK_SOCKET", &socket)
        .env("GHBRK_POLICY", &policy)
        .env("GHBRK_AUDIT_LOG", &audit)
        .spawn()
        .expect("spawn ghbrk daemon");

    // Wait for the socket to appear.
    let mut found = false;
    for _ in 0..200 {
        if socket.exists() {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if !found {
        let _ = child.kill();
        panic!("daemon socket never appeared at {}", socket.display());
    }

    // Send SIGTERM.
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).expect("send SIGTERM");

    // Wait up to 5s for clean exit.
    let mut exited = None;
    for _ in 0..200 {
        match child.try_wait().unwrap() {
            Some(status) => {
                exited = Some(status);
                break;
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    let status = match exited {
        Some(s) => s,
        None => {
            let _ = child.kill();
            panic!("daemon did not exit within 5s of SIGTERM");
        }
    };

    assert!(
        status.success(),
        "daemon exit was not zero: {:?}",
        status.code()
    );
    assert!(!socket.exists(), "socket file was not removed on shutdown");
    // Audit file should still be on disk; flush ran before exit.
    assert!(audit.exists(), "audit file missing after shutdown");
}
