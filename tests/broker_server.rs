//! Integration tests for the broker server.
//!
//! Each test starts a broker bound to a temp Unix socket, exercises a single
//! invariant, then cleans up. Tests rely on a real Tokio runtime + real Unix
//! socket — we are testing wire behaviour, not a mock.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
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
    audit_path: PathBuf,
    handle: tokio::task::JoinHandle<()>,
    /// A clone of the broker's swappable policy handle. Tests use this seam to
    /// hot-reload the policy without touching the file system.
    policy: Arc<ArcSwap<Policy>>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with_creds(None).await
    }

    async fn start_with_creds(credentials_root: Option<PathBuf>) -> Self {
        Self::start_with(dummy_policy(), credentials_root).await
    }

    async fn start_with(policy: Policy, credentials_root: Option<PathBuf>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let socket_path = tmp.path().join("broker.sock");
        let audit_path = tmp.path().join("audit.log");
        let policy_path = tmp.path().join("policy.yaml");
        let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
        let policy_handle = Arc::new(ArcSwap::from_pointee(policy));
        let config = BrokerConfig {
            socket_path: socket_path.clone(),
            policy: Arc::clone(&policy_handle),
            policy_path,
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
            audit_path,
            handle,
            policy: policy_handle,
        }
    }

    /// Hot-reload the broker's policy via the swappable handle. Connections
    /// accepted after this call observe the new policy snapshot.
    fn swap_policy(&self, policy: Policy) {
        self.policy.store(Arc::new(policy));
    }
}

/// Read all `ServerFrame`s from `stream` until an `Exit` frame is seen,
/// concatenating any `StdoutChunk` payloads into a single string. Returns the
/// stdout text and the exit code.
async fn collect_until_exit(stream: &mut UnixStream) -> (String, i32) {
    let mut out = Vec::new();
    loop {
        let frame: ServerFrame = read_frame(stream).await.expect("frame");
        match frame {
            ServerFrame::StdoutChunk { data } => out.extend_from_slice(&data),
            ServerFrame::Exit { code } => {
                return (String::from_utf8_lossy(&out).into_owned(), code);
            }
            other => panic!("unexpected frame before Exit: {other:?}"),
        }
    }
}

/// Open a fresh connection, send a `Tool::Policy` query for `repo_spec`, and
/// return the operations listed in the report's allowed section. Each call is a
/// brand-new connection so it observes the policy snapshot current at accept
/// time.
async fn query_allowed_ops(socket_path: &std::path::Path, repo_spec: &str) -> String {
    let mut stream = UnixStream::connect(socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Policy,
        args: vec![repo_spec.into()],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let (report, _code) = collect_until_exit(&mut stream).await;
    // The report has an "allowed operations:" section followed by a
    // "forbidden operations" section; isolate the allowed slice so an op
    // appearing in the forbidden list never reads as allowed.
    report
        .split("allowed operations:")
        .nth(1)
        .unwrap_or("")
        .split("forbidden operations")
        .next()
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn policy_reload_visible_to_new_connections() {
    // Start under a deny-all policy: a policy query for acme/web lists every
    // operation as forbidden, so `push` is not in the allowed section.
    let h = Harness::start_with(dummy_policy(), None).await;
    let before = query_allowed_ops(&h.socket_path, "acme/web").await;
    assert!(
        !before.contains("push"),
        "deny-all policy unexpectedly allowed push before reload:\n{before}"
    );

    // Hot-swap to a policy that allows push for acme/web via the test seam.
    let allow_push = Policy::from_yaml(
        "rules:\n  \
         - user: \"*\"\n    \
           org: \"acme\"\n    \
           repo: \"web\"\n    \
           branches: [\"*\"]\n    \
           operations: [push]\n    \
           effect: allow\n",
    )
    .unwrap();
    h.swap_policy(allow_push);

    // A brand-new connection must observe the swapped policy: push is now
    // listed in the allowed-operations section.
    let after = query_allowed_ops(&h.socket_path, "acme/web").await;
    assert!(
        after.contains("push"),
        "new connection did not observe the reloaded policy; allowed ops:\n{after}"
    );

    h.handle.abort();
}

#[tokio::test]
async fn allow_request_routed_and_denied_for_non_root_peer() {
    // The broker routes Tool::Allow to its allow handler before resolve/policy.
    // The test runner is non-root, so the privilege gate must deny it over the
    // wire. (Root-peer append/reload is covered by the handle_allow seam in
    // tests/allow_command.rs.)
    if nix::unistd::geteuid().is_root() {
        eprintln!("running as root; skipping non-root allow-deny assertion");
        return;
    }
    let h = Harness::start().await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Allow,
        args: vec!["acme/web".into(), "write".into()],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let frame: ServerFrame = read_frame(&mut stream).await.unwrap();
    match frame {
        ServerFrame::Denied { reason } => {
            assert!(
                reason.to_lowercase().contains("privilege")
                    || reason.to_lowercase().contains("root"),
                "deny reason should mention privilege: {reason}"
            );
        }
        other => panic!("expected Denied for non-root allow, got {other:?}"),
    }
    assert!(!h.handle.is_finished(), "broker died handling allow");
    h.handle.abort();
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

#[tokio::test]
async fn explain_local_git_reports_out_of_scope() {
    let h = Harness::start().await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Explain,
        args: vec!["git".into(), "status".into()],
        cwd: PathBuf::from("/work/repo"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let (text, code) = collect_until_exit(&mut stream).await;
    assert_eq!(code, 0, "explain of a local subcommand exits 0");
    assert!(text.contains("local"), "expected 'local' in:\n{text}");
    assert!(
        text.contains("ghbrk only brokers"),
        "expected guidance in:\n{text}"
    );
    h.handle.abort();
}

#[tokio::test]
async fn explain_remote_git_reports_policy_and_inject() {
    let policy = Policy::from_yaml(
        "rules:\n  - user: \"*\"\n    org: \"*\"\n    repo: \"*\"\n    operations: [push]\n    branches: [\"*\"]\n    effect: allow\n",
    )
    .unwrap();
    let h = Harness::start_with(policy, None).await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Explain,
        args: vec!["git".into(), "push".into(), "origin".into(), "main".into()],
        cwd: PathBuf::from("/work/repo"),
        remote_url: Some("git@github.com:acme/web.git".into()),
        head_branch: Some("main".into()),
    };
    write_frame(&mut stream, &req).await.unwrap();
    let (text, code) = collect_until_exit(&mut stream).await;
    assert_eq!(code, 0);
    assert!(text.contains("acme/web"), "expected repo in:\n{text}");
    assert!(text.contains("push"), "expected operation in:\n{text}");
    assert!(text.contains("allow"), "expected allow in:\n{text}");
    assert!(
        text.contains("SSH credential"),
        "expected SSH inject in:\n{text}"
    );
    h.handle.abort();
}

#[tokio::test]
async fn policy_lists_allowed_and_forbidden() {
    let policy = Policy::from_yaml(
        "rules:\n  - user: \"*\"\n    org: acme\n    repo: web\n    operations: [push, pr_open]\n    branches: [\"*\"]\n    effect: allow\n",
    )
    .unwrap();
    let h = Harness::start_with(policy, None).await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Policy,
        args: vec!["acme/web".into()],
        cwd: PathBuf::from("/work/repo"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let (text, code) = collect_until_exit(&mut stream).await;
    assert_eq!(code, 0);
    assert!(text.contains("acme/web"), "expected repo header:\n{text}");
    assert!(
        text.contains("allowed operations:"),
        "expected allowed group:\n{text}"
    );
    assert!(
        text.contains("forbidden operations"),
        "expected forbidden group:\n{text}"
    );
    assert!(text.contains("push"), "push should be allowed:\n{text}");
    assert!(
        text.contains("pr_open"),
        "pr_open should be allowed:\n{text}"
    );
    assert!(text.contains("fetch"), "fetch should be forbidden:\n{text}");
    h.handle.abort();
}

#[tokio::test]
async fn policy_rejects_malformed_repo_specifier() {
    let h = Harness::start().await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Policy,
        args: vec!["not-a-repo".into()],
        cwd: PathBuf::from("/work/repo"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let (text, code) = collect_until_exit(&mut stream).await;
    assert_eq!(code, 1);
    assert!(text.contains("invalid repo specifier"), "got:\n{text}");
    h.handle.abort();
}

#[tokio::test]
async fn broker_denies_local_git_subcommand() {
    let h = Harness::start().await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Git,
        args: vec!["status".into()],
        cwd: PathBuf::from("/work/repo"),
        remote_url: None,
        head_branch: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let frame: ServerFrame = read_frame(&mut stream).await.unwrap();
    let reason = match frame {
        ServerFrame::Denied { reason } => reason,
        other => panic!("expected Denied, got {other:?}"),
    };
    assert!(
        reason.contains("local git operations must be run directly"),
        "got reason: {reason}"
    );

    // The broker must record a deny entry in the audit log.
    h.handle.abort();
    let _ = h.handle.await;
    let log = std::fs::read_to_string(&h.audit_path).expect("audit log readable");
    assert!(
        log.contains("\"decision\":\"deny\"") && log.contains("status"),
        "expected a deny entry mentioning the subcommand, got:\n{log}"
    );
}
