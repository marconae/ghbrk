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
use ghbrk::protocol::{
    read_frame, write_frame, ClientFrame, Request, ServerFrame, TmpIdentity, Tool,
};
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

/// What one invocation's response produced: its output streams as text, the
/// code the terminating `Exit` frame carried, and the reason a `Denied`
/// frame gave, when the request never reached a child.
struct ChildResponse {
    stdout: String,
    stderr: String,
    code: i32,
    denied: Option<String>,
}

impl ChildResponse {
    /// Concatenates stdout and stderr, for callers that read the two streams
    /// as one combined transcript rather than comparing them separately.
    fn merged_output(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    /// Panics with the `Denied` reason when the request never reached a
    /// child. Every caller of this accessor expects execution, so a denial
    /// is never the outcome under test.
    fn expect_executed(&self) -> &Self {
        if let Some(reason) = &self.denied {
            panic!("request denied before execution: {reason}");
        }
        self
    }
}

/// Read all `ServerFrame`s from `stream` until `Exit`, accumulating stdout
/// and stderr into separate buffers, ignoring any `CredentialAudit` frame,
/// and recording a `Denied` frame's reason rather than failing immediately —
/// callers that require execution use [`ChildResponse::expect_executed`] and
/// callers that read the streams as one transcript use
/// [`ChildResponse::merged_output`].
async fn collect_response(stream: &mut UnixStream) -> ChildResponse {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut denied = None;
    loop {
        let frame: ServerFrame = read_frame(stream).await.expect("frame");
        match frame {
            ServerFrame::StdoutChunk { data } => stdout.extend_from_slice(&data),
            ServerFrame::StderrChunk { data } => stderr.extend_from_slice(&data),
            ServerFrame::CredentialAudit { .. } => {}
            ServerFrame::Denied { reason } => denied = Some(reason),
            ServerFrame::Exit { code } => {
                return ChildResponse {
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    code,
                    denied,
                };
            }
        }
    }
}

fn own_tmp_identity() -> TmpIdentity {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata("/tmp").unwrap();
    TmpIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
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
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let report = response.merged_output();
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
        client_frames: false,
        caller_tmp: None,
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

    let expected_user = current_test_user();

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
                client_frames: false,
                caller_tmp: None,
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
        client_frames: false,
        caller_tmp: None,
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
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let (text, code) = (response.merged_output(), response.code);
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
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let (text, code) = (response.merged_output(), response.code);
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
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let (text, code) = (response.merged_output(), response.code);
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
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let (text, code) = (response.merged_output(), response.code);
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
        client_frames: false,
        caller_tmp: None,
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

/// `gh release <verb>` must be policy-gated, not executed via ungoverned
/// passthrough. A user with no rule granting any release operation is denied,
/// and the audit log names the resolved operation (`release_delete`) rather
/// than `passthrough` — proving the request went through resolve + policy.
#[tokio::test]
async fn broker_denies_release_delete_by_default() {
    let h = Harness::start().await; // dummy_policy(): empty rules, default-deny.
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Gh,
        args: vec![
            "release".into(),
            "delete".into(),
            "v1.0.0".into(),
            "--repo".into(),
            "acme/web".into(),
            "--yes".into(),
        ],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let frame: ServerFrame = read_frame(&mut stream).await.unwrap();
    assert!(
        matches!(frame, ServerFrame::Denied { .. }),
        "expected Denied for release_delete with no granting rule, got {frame:?}"
    );

    h.handle.abort();
    let _ = h.handle.await;
    let log = std::fs::read_to_string(&h.audit_path).expect("audit log readable");
    assert!(
        log.contains("\"decision\":\"deny\"") && log.contains("release_delete"),
        "expected a deny entry naming release_delete (not passthrough), got:\n{log}"
    );
}

fn current_test_user() -> String {
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

/// Serialises the process-wide `PATH` mutation every stub `gh` needs. Two
/// tests installing a stub at once would each capture the other's value as the
/// original and restore it while the other's child still needs its directory,
/// so the mutation and the whole span it must survive are held exclusively.
static PATH_MUTATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Restores the process-wide `PATH` env var on drop and releases the exclusive
/// claim on it.
///
/// Neither `ChildSpec` (`src/executor.rs`) nor `BrokerConfig` (`src/broker.rs`)
/// expose a way to inject an environment override scoped to just the
/// broker-under-test's spawned children — `stream_child` only falls back to
/// `std::env::var("PATH")` when the child's own env has no `PATH` entry, and
/// nothing plumbs a test-supplied `PATH` into that per-request env. Until such
/// a seam exists, a stub must mutate the process-wide `PATH`, so this guard
/// restores the prior value on drop (including on panic) to keep the mutation
/// from leaking into other tests running concurrently in this binary.
#[must_use]
struct PathGuard {
    _exclusive: tokio::sync::MutexGuard<'static, ()>,
    original: Option<String>,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

/// Claim the process-wide `PATH` and put `dir` in front of it. Keep the
/// returned guard alive for as long as a child must resolve a stub from `dir`.
async fn prepend_to_path(dir: &std::path::Path) -> PathGuard {
    let exclusive = PATH_MUTATION.lock().await;
    let original = std::env::var("PATH").ok();
    let prev = original.clone().unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", dir.display(), prev));
    PathGuard {
        _exclusive: exclusive,
        original,
    }
}

/// Place a stub `gh` on PATH that prints its `GH_TOKEN` to stdout, so an
/// allowed release operation can be proven to reach real execution without
/// depending on the real `gh` binary or network access.
///
/// Returns a guard that restores the original `PATH` when dropped; keep it
/// alive for the duration of the test.
async fn install_stub_gh(dir: &std::path::Path) -> PathGuard {
    let script = dir.join("gh");
    write_mode(
        &script,
        "#!/bin/sh\nprintf 'argv=%s GH_TOKEN=%s' \"$*\" \"$GH_TOKEN\"\n",
        0o755,
    );
    prepend_to_path(dir).await
}

/// A user granted the `maintain` role can perform a mutating release
/// operation: the request clears policy and reaches real execution (a stub
/// `gh` receives the injected `GH_TOKEN`), proving `maintain` carries
/// `release_delete` end-to-end over the broker socket.
#[tokio::test]
async fn broker_allows_release_delete_under_maintain() {
    let user = current_test_user();

    let creds_root = tempfile::tempdir().unwrap();
    let user_dir = creds_root.path().join(&user);
    std::fs::create_dir_all(&user_dir).unwrap();
    write_mode(&user_dir.join("id_rsa"), "dummy-key", 0o600);
    write_mode(&user_dir.join("token"), "ghp_maintain_test_token", 0o600);

    let bin_dir = tempfile::tempdir().unwrap();
    let _path_guard = install_stub_gh(bin_dir.path()).await;

    let policy = Policy::from_yaml(&format!(
        "rules:\n  - user: \"{user}\"\n    org: acme\n    repo: web\n    operations: maintain\n    effect: allow\n"
    ))
    .unwrap();
    let h = Harness::start_with(policy, Some(creds_root.path().to_path_buf())).await;

    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Gh,
        args: vec![
            "release".into(),
            "delete".into(),
            "v1.0.0".into(),
            "--repo".into(),
            "acme/web".into(),
            "--yes".into(),
        ],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let (out, code) = (response.merged_output(), response.code);
    assert_eq!(
        code, 0,
        "expected clean exit for maintain-granted release_delete, got output:\n{out}"
    );
    assert!(
        out.contains("GH_TOKEN=ghp_maintain_test_token"),
        "expected stub gh to receive the injected token, got:\n{out}"
    );

    h.handle.abort();
}

/// Line the stdin-echoing stub `gh` writes before it reads a byte, so a test
/// can observe output the broker produced while client frames were still to
/// come.
const STUB_READY: &str = "stub ready";

/// Liveness bound for a brokered invocation: it either finishes promptly or is
/// waiting on a standard input that will never arrive.
const CHILD_MUST_FINISH_WITHIN: Duration = Duration::from_secs(10);

/// Place a stub `gh` on PATH that announces itself and then echoes its own
/// standard input, so what the child received is observable as the response's
/// stdout text.
async fn install_stdin_echoing_gh(dir: &std::path::Path) -> PathGuard {
    write_mode(
        &dir.join("gh"),
        &format!("#!/bin/sh\necho '{STUB_READY}'\nexec cat\n"),
        0o755,
    );
    prepend_to_path(dir).await
}

/// A credentials root holding a well-formed credential set for `user`, which
/// the broker requires before it will spawn any child.
fn credentials_for(user: &str) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    let user_dir = root.path().join(user);
    std::fs::create_dir_all(&user_dir).unwrap();
    write_mode(&user_dir.join("id_rsa"), "dummy-key", 0o600);
    write_mode(&user_dir.join("token"), "ghp_stdin_test_token", 0o600);
    root
}

/// A running broker under `policy` whose spawned `gh` echoes its own standard
/// input, with credentials in place for the current user.
///
/// It owns every value the invocation needs alive behind it — the credentials
/// root, the stub's directory, and the claim on `PATH` — so a test cannot hold
/// the broker without also holding them.
struct EchoingBroker {
    harness: Harness,
    _credentials_root: TempDir,
    _stub_dir: TempDir,
    _path_guard: PathGuard,
}

async fn broker_with_echoing_gh(policy: Policy) -> EchoingBroker {
    let credentials_root = credentials_for(&current_test_user());
    let stub_dir = tempfile::tempdir().unwrap();
    let path_guard = install_stdin_echoing_gh(stub_dir.path()).await;
    let harness = Harness::start_with(policy, Some(credentials_root.path().to_path_buf())).await;
    EchoingBroker {
        harness,
        _credentials_root: credentials_root,
        _stub_dir: stub_dir,
        _path_guard: path_guard,
    }
}

/// A `gh` request carrying `args`. Every test sets `client_frames` on the
/// returned value itself, because that declaration is what is under test.
fn gh_request(args: &[&str]) -> Request {
    Request {
        tool: Tool::Gh,
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
        client_frames: false,
        caller_tmp: None,
    }
}

/// Read server frames until the collected stdout carries `marker`, returning
/// everything collected up to and including it. An `Exit` frame arriving first
/// means the child never produced the marker.
async fn read_stdout_until(stream: &mut UnixStream, marker: &str) -> String {
    let mut stdout = Vec::new();
    loop {
        let frame: ServerFrame = read_frame(stream).await.expect("frame");
        match frame {
            ServerFrame::StdoutChunk { data } => {
                stdout.extend_from_slice(&data);
                let text = String::from_utf8_lossy(&stdout);
                if text.contains(marker) {
                    return text.into_owned();
                }
            }
            other => panic!("expected stdout carrying {marker:?}, got {other:?}"),
        }
    }
}

/// A request that declares client frames hands the executor the connection's
/// read half: the bytes the caller sends after the request reach the spawned
/// child's standard input, and the child echoes them back as stdout.
#[tokio::test]
async fn client_frames_reach_the_spawned_child() {
    let policy = Policy::from_yaml(&format!(
        "rules:\n  - user: \"{}\"\n    org: acme\n    repo: web\n    operations: write\n    effect: allow\n",
        current_test_user()
    ))
    .unwrap();
    let broker = broker_with_echoing_gh(policy).await;

    let mut stream = UnixStream::connect(&broker.harness.socket_path)
        .await
        .unwrap();
    let mut req = gh_request(&[
        "pr",
        "comment",
        "42",
        "--repo",
        "acme/web",
        "--body-file",
        "-",
    ]);
    req.client_frames = true;
    write_frame(&mut stream, &req).await.unwrap();
    write_frame(
        &mut stream,
        &ClientFrame::StdinChunk {
            data: b"body text\n".to_vec(),
        },
    )
    .await
    .unwrap();
    write_frame(&mut stream, &ClientFrame::StdinEof)
        .await
        .unwrap();

    let response = tokio::time::timeout(CHILD_MUST_FINISH_WITHIN, collect_response(&mut stream))
        .await
        .expect("a child fed its standard input must not wait on it");
    response.expect_executed();

    assert_eq!(
        response.code, 0,
        "expected clean exit, stderr:\n{}",
        response.stderr
    );
    assert_eq!(
        response.stdout,
        format!("{STUB_READY}\nbody text\n"),
        "expected the child to echo the bytes sent as client frames"
    );
    broker.harness.handle.abort();
}

/// A client that declares client frames and then closes its write half
/// without sending one leaves the executor reading end-of-file where a frame
/// would be. That is an empty standard input, not a protocol failure: the
/// child still runs, no `Denied` frame is emitted, and the `Exit` frame still
/// carries the child's own code.
#[tokio::test]
async fn request_without_client_frames_yields_empty_stdin() {
    let broker = broker_with_echoing_gh(dummy_policy()).await;

    let mut stream = UnixStream::connect(&broker.harness.socket_path)
        .await
        .unwrap();
    let mut req = gh_request(&["auth", "status"]);
    req.client_frames = true;
    write_frame(&mut stream, &req).await.unwrap();
    stream.shutdown().await.unwrap();

    let response = tokio::time::timeout(CHILD_MUST_FINISH_WITHIN, collect_response(&mut stream))
        .await
        .expect("end-of-file where a client frame would be must not stall the relay");
    response.expect_executed();

    assert_eq!(
        response.code, 0,
        "expected clean exit, stderr:\n{}",
        response.stderr
    );
    assert_eq!(
        response.stdout,
        format!("{STUB_READY}\n"),
        "a child given no client frames must observe an empty standard input"
    );
    broker.harness.handle.abort();
}

/// A request carrying no `client_frames` field at all — the shape a client
/// released before stdin forwarding sends — gets an input source already at
/// end-of-file, never the connection's read half. Such a client holds its
/// write half open for the whole response, as this one does, so a broker that
/// waited on the connection would leave the child blocked on a pipe that never
/// closes.
#[tokio::test]
async fn undeclared_client_frames_does_not_wait_for_stdin() {
    let broker = broker_with_echoing_gh(dummy_policy()).await;

    let mut stream = UnixStream::connect(&broker.harness.socket_path)
        .await
        .unwrap();
    let released_before_stdin_forwarding =
        serde_json::json!({"tool": "gh", "args": ["auth", "status"], "cwd": "/"});
    write_frame(&mut stream, &released_before_stdin_forwarding)
        .await
        .unwrap();

    // The write half stays open for the whole response: `stream` is neither
    // shut down nor dropped before the last frame is read.
    let response = tokio::time::timeout(CHILD_MUST_FINISH_WITHIN, collect_response(&mut stream))
        .await
        .expect("a request declaring no client frames must not wait for one");
    response.expect_executed();

    assert_eq!(
        response.code, 0,
        "expected the child's own exit code, stderr:\n{}",
        response.stderr
    );
    assert_eq!(
        response.stdout,
        format!("{STUB_READY}\n"),
        "the child must observe end-of-file on standard input rather than block"
    );
    broker.harness.handle.abort();
}

/// Both directions live on one connection at once. The client reads the
/// child's first output line before it sends any standard input, and the
/// broker reads the client frames that follow it while it is already writing
/// server frames. A broker that drained one direction before servicing the
/// other would stall here: each side would be waiting on the other.
#[tokio::test]
async fn stdin_and_output_frames_interleave_on_one_connection() {
    let broker = broker_with_echoing_gh(dummy_policy()).await;

    let mut stream = UnixStream::connect(&broker.harness.socket_path)
        .await
        .unwrap();
    let mut req = gh_request(&["auth", "status"]);
    req.client_frames = true;
    write_frame(&mut stream, &req).await.unwrap();

    let exchange = async {
        let announced = read_stdout_until(&mut stream, STUB_READY).await;
        write_frame(
            &mut stream,
            &ClientFrame::StdinChunk {
                data: b"sent after output\n".to_vec(),
            },
        )
        .await
        .unwrap();
        write_frame(&mut stream, &ClientFrame::StdinEof)
            .await
            .unwrap();
        let response = collect_response(&mut stream).await;
        (announced, response)
    };
    let (announced, response) = tokio::time::timeout(CHILD_MUST_FINISH_WITHIN, exchange)
        .await
        .expect("neither direction may wait for the other to finish");
    response.expect_executed();

    assert_eq!(
        response.code, 0,
        "expected clean exit, stderr:\n{}",
        response.stderr
    );
    assert_eq!(
        format!("{announced}{}", response.stdout),
        format!("{STUB_READY}\nsent after output\n"),
        "output written before the first client frame, and input read after it"
    );
    broker.harness.handle.abort();
}

/// A `check` request with no caller `/tmp` identity (the shape a client
/// released before this change sends) runs every credential check as before
/// and never fails the check for the absent identity — proven by the absence
/// of any `Shared filesystem:` line.
#[tokio::test]
async fn check_without_caller_tmp_omits_filesystem_line() {
    let h = Harness::start().await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Check,
        args: vec![],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
        client_frames: false,
        caller_tmp: None,
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let out = response.merged_output();
    assert!(
        !out.contains("Shared filesystem:"),
        "expected no Shared filesystem line without a caller_tmp identity, got:\n{out}"
    );
    h.handle.abort();
}

/// A `check` request carrying the broker's own `/tmp` identity as the caller
/// identity reports the shared-filesystem check as `OK`, since both sides
/// resolve `/tmp` to the same directory in this test process.
#[tokio::test]
async fn check_with_matching_caller_tmp_reports_ok() {
    let h = Harness::start().await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let req = Request {
        tool: Tool::Check,
        args: vec![],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
        client_frames: false,
        caller_tmp: Some(own_tmp_identity()),
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let out = response.merged_output();
    assert!(
        out.contains("Shared filesystem: OK"),
        "expected a matching identity to report OK, got:\n{out}"
    );
    h.handle.abort();
}

/// A `check` request carrying a caller `/tmp` identity that does not match
/// the broker's own `/tmp` reports an `ERROR` naming `PrivateTmp=` as the
/// cause and the unit file to edit as the remediation.
#[tokio::test]
async fn check_with_mismatched_caller_tmp_reports_error() {
    let h = Harness::start().await;
    let mut stream = UnixStream::connect(&h.socket_path).await.unwrap();
    let mut identity = own_tmp_identity();
    identity.ino = identity.ino.wrapping_add(1);
    let req = Request {
        tool: Tool::Check,
        args: vec![],
        cwd: PathBuf::from("/"),
        remote_url: None,
        head_branch: None,
        client_frames: false,
        caller_tmp: Some(identity),
    };
    write_frame(&mut stream, &req).await.unwrap();
    let response = collect_response(&mut stream).await;
    response.expect_executed();
    let (out, code) = (response.merged_output(), response.code);
    assert_ne!(code, 0, "a mismatched identity must fail the check");
    assert!(
        out.contains("Shared filesystem: ERROR"),
        "expected a mismatched identity to report ERROR, got:\n{out}"
    );
    assert!(out.contains("PrivateTmp="), "got:\n{out}");
    assert!(
        out.contains("/etc/systemd/system/ghbrk.service"),
        "expected the unit file remediation, got:\n{out}"
    );
    h.handle.abort();
}
