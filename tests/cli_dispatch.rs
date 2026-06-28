use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;

use ghbrk::protocol::{read_frame, write_frame, ClientFrame, Request, ServerFrame};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

/// Everything one brokered invocation put on the client-to-broker direction.
struct Forwarded {
    request: Request,
    frames: Vec<ClientFrame>,
    /// A frame arrived that the request never declared.
    undeclared_frame: bool,
}

/// Bind a stub broker at `socket` and serve exactly one invocation: read the
/// request, drain any declared client frames up to `StdinEof`, then report a
/// zero exit. Binding happens synchronously before the handle is returned, so
/// the spawned binary cannot race the listener; the bound listener is then
/// adopted by a `tokio::runtime::Runtime` run from an ordinary blocking
/// thread, since these tests drive the installed binary rather than an async
/// caller, and reuses `ghbrk::protocol`'s own framing rather than a second
/// hand-rolled copy of it.
fn stub_broker(socket: &Path) -> thread::JoinHandle<Forwarded> {
    let listener = UnixListener::bind(socket).expect("bind stub broker");
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            listener.set_nonblocking(true).expect("set nonblocking");
            let listener =
                tokio::net::UnixListener::from_std(listener).expect("adopt stub listener");
            let (mut stream, _) = listener.accept().await.expect("accept");
            let request: Request = read_frame(&mut stream).await.expect("request frame");
            let mut frames = Vec::new();
            if request.client_frames {
                while let Ok(frame) = read_frame::<_, ClientFrame>(&mut stream).await {
                    let is_eof = frame == ClientFrame::StdinEof;
                    frames.push(frame);
                    if is_eof {
                        break;
                    }
                }
            }
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            // The gateway closes the connection once it has the exit code, so
            // this read ends rather than blocking. Anything readable here is a
            // frame the request never announced.
            let undeclared_frame = read_frame::<_, ClientFrame>(&mut stream).await.is_ok();
            Forwarded {
                request,
                frames,
                undeclared_frame,
            }
        })
    })
}

fn stdin_bytes(frames: &[ClientFrame]) -> Vec<u8> {
    frames
        .iter()
        .filter_map(|frame| match frame {
            ClientFrame::StdinChunk { data } => Some(data.as_slice()),
            ClientFrame::StdinEof => None,
        })
        .flatten()
        .copied()
        .collect()
}

/// Run the installed binary against `socket` with `stdin` delivered on a pipe
/// that closes once written, the shape a shell gives `cmd | ghbrk …`.
fn run_with_piped_stdin(socket: &Path, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(bin())
        .args(args)
        .env("GHBRK_SOCKET", socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ghbrk");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(stdin)
        .expect("write to stdin pipe");
    child.wait_with_output().expect("wait for ghbrk")
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
fn gh_forwards_piped_stdin_to_broker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = tmp.path().join("broker.sock");
    let broker = stub_broker(&socket);

    let out = run_with_piped_stdin(
        &socket,
        &["gh", "pr", "create", "--body-file", "-"],
        b"body text\n",
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let forwarded = broker.join().expect("stub broker");
    assert!(forwarded.request.client_frames);
    assert_eq!(
        forwarded.request.args,
        vec!["pr", "create", "--body-file", "-"]
    );
    assert_eq!(stdin_bytes(&forwarded.frames), b"body text\n");
    assert_eq!(forwarded.frames.last(), Some(&ClientFrame::StdinEof));
}

#[test]
fn git_forwards_piped_stdin_to_broker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = tmp.path().join("broker.sock");
    let broker = stub_broker(&socket);
    let refspec = b"refs/heads/main:refs/remotes/origin/main\n";

    let out = run_with_piped_stdin(&socket, &["git", "fetch", "origin", "--stdin"], refspec);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let forwarded = broker.join().expect("stub broker");
    assert!(forwarded.request.client_frames);
    assert_eq!(forwarded.request.args, vec!["fetch", "origin", "--stdin"]);
    assert_eq!(stdin_bytes(&forwarded.frames), refspec);
    assert_eq!(forwarded.frames.last(), Some(&ClientFrame::StdinEof));
}

#[test]
fn explain_forwards_no_stdin() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let socket = tmp.path().join("broker.sock");
    let broker = stub_broker(&socket);

    let out = run_with_piped_stdin(
        &socket,
        &["explain", "git", "push", "origin", "main"],
        &[b'x'; 1024],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let forwarded = broker.join().expect("stub broker");
    assert!(!forwarded.request.client_frames);
    assert!(forwarded.frames.is_empty());
    assert!(
        !forwarded.undeclared_frame,
        "explain must send the request and nothing else"
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
