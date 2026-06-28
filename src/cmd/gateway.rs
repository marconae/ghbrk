//! Broker-relay transport for the explicit `ghbrk git` / `ghbrk gh` gateways.
//!
//! Connects directly to the broker socket, writes a `Request` frame, streams
//! the caller's own standard input to the broker behind it, and streams the
//! broker's `ServerFrame` responses to the process's real stdio. The two
//! directions run concurrently and the relay ends on the response side only:
//! a child that writes output while it waits for more input would deadlock a
//! client that finished sending before it started reading, and so would a
//! daemon released before stdin forwarding existed, which never drains the
//! stdin direction at all.
//!
//! Unlike the former transparent shim, there is no config, no passthrough
//! exec, and no EACCES silent fall-through: if the broker cannot be reached,
//! the gateway reports the failure and exits non-zero.

use std::env;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::process;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use ghbrk::protocol::{
    read_frame, write_frame, ClientFrame, ProtocolError, Request, ServerFrame, Tool,
};

/// Default broker socket path, overridable via `GHBRK_SOCKET`.
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/ghbrk/broker.sock";

/// Environment variable that overrides the default broker socket path.
pub const SOCKET_ENV_VAR: &str = "GHBRK_SOCKET";

/// Exit code used when the broker cannot be reached or the protocol fails.
pub const GATEWAY_ERROR_EXIT: i32 = 1;

/// Bytes carried by one `StdinChunk` frame. Held at the executor's own read
/// bound so one read on the caller's side becomes one write on the child's,
/// rather than a frame whose size the ceiling on frame length has to police.
const STDIN_CHUNK_SIZE: usize = ghbrk::executor::READ_BUF_SIZE;

/// Resolve the broker socket path, honouring `GHBRK_SOCKET` when set.
pub fn socket_path_from_env() -> PathBuf {
    env::var_os(SOCKET_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH))
}

/// Relay `tool` + `args` to the broker at `socket_path` and terminate the
/// process with the resulting exit code. Never returns.
pub fn run_gateway(
    tool: Tool,
    args: Vec<String>,
    cwd: PathBuf,
    socket_path: &Path,
    remote_url: Option<String>,
    head_branch: Option<String>,
) -> ! {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("ghbrk: failed to start async runtime: {err}");
            process::exit(GATEWAY_ERROR_EXIT);
        }
    };

    let code = runtime.block_on(async move {
        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();
        relay(
            tool,
            args,
            cwd,
            socket_path,
            remote_url,
            head_branch,
            &mut stdout,
            &mut stderr,
        )
        .await
    });
    process::exit(code);
}

/// Core async relay, generic over the stdio writers so it can be tested
/// against in-memory buffers. Returns the exit code the caller should use.
#[allow(clippy::too_many_arguments)]
pub(super) async fn relay<O, E>(
    tool: Tool,
    args: Vec<String>,
    cwd: PathBuf,
    socket_path: &Path,
    remote_url: Option<String>,
    head_branch: Option<String>,
    stdout: &mut O,
    stderr: &mut E,
) -> i32
where
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    let stdin = caller_stdin(tool, std::io::stdin().is_terminal());
    let request = Request {
        tool,
        args,
        cwd,
        remote_url,
        head_branch,
        client_frames: stdin.is_some(),
        caller_tmp: None,
    };
    relay_request(request, socket_path, stdin, stdout, stderr).await
}

/// Standard input the gateway forwards for `tool`.
///
/// `git` and `gh` spawn a child that may read standard input, so the caller's
/// own is streamed to it. An interactive terminal is swapped for an empty
/// source before any relay exists, so no code path can reach the terminal and
/// capture keystrokes the user meant for their own shell, while the child
/// still observes end-of-file rather than waiting. Every other tool spawns no
/// child, forwards nothing, and by returning `None` here also declares no
/// client frames on the wire.
fn caller_stdin(tool: Tool, stdin_is_terminal: bool) -> Option<Box<dyn AsyncRead + Unpin>> {
    match (tool, stdin_is_terminal) {
        (Tool::Git | Tool::Gh, false) => Some(Box::new(tokio::io::stdin())),
        (Tool::Git | Tool::Gh, true) => Some(Box::new(tokio::io::empty())),
        (Tool::Check | Tool::Explain | Tool::Policy | Tool::Allow, _) => None,
    }
}

/// Connect, send `request`, and relay both directions until the broker reports
/// an exit code.
///
/// `stdin` present means client frames follow the request, which is the same
/// fact `request.client_frames` declares to the broker; absent means the
/// request is the only frame this direction ever carries, and the write half
/// stays open for the whole response exactly as it did before stdin forwarding
/// existed. Only the response direction can end the relay: the stdin direction
/// may never end at all.
async fn relay_request<O, E, S>(
    request: Request,
    socket_path: &Path,
    stdin: Option<S>,
    stdout: &mut O,
    stderr: &mut E,
) -> i32
where
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
    S: AsyncRead + Unpin,
{
    let stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        Err(err) => {
            let msg = format!(
                "ghbrk: cannot connect to broker at {}: {}\n",
                socket_path.display(),
                err
            );
            write_then_flush(stderr, msg.as_bytes()).await;
            return GATEWAY_ERROR_EXIT;
        }
    };

    let (read_half, mut write_half) = stream.into_split();

    if let Err(err) = write_frame(&mut write_half, &request).await {
        let msg = format!("ghbrk: failed to send request to broker: {err}\n");
        write_then_flush(stderr, msg.as_bytes()).await;
        return GATEWAY_ERROR_EXIT;
    }

    let Some(source) = stdin else {
        return read_responses(read_half, stdout, stderr).await;
    };

    let mut pump = pin!(pump_stdin(write_half, source));
    let mut responses = pin!(read_responses(read_half, stdout, stderr));
    let mut pumping = true;
    loop {
        tokio::select! {
            code = &mut responses => return code,
            _ = &mut pump, if pumping => pumping = false,
        }
    }
}

/// Stream `source` to the broker as `StdinChunk` frames and close that
/// direction behind them, so the child observes end-of-file exactly when the
/// caller's own standard input does.
///
/// Every exit path shuts the direction down, including the one where a write
/// failure loses the `StdinEof` frame: a broker that never sees end-of-file
/// leaves the child waiting for input that cannot arrive. A failure to read
/// the caller's standard input is end-of-file too, for the same reason.
async fn pump_stdin<W, S>(mut writer: W, mut source: S)
where
    W: AsyncWrite + Unpin,
    S: AsyncRead + Unpin,
{
    let mut buf = vec![0u8; STDIN_CHUNK_SIZE];
    loop {
        let read = match source.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = ClientFrame::StdinChunk {
            data: buf[..read].to_vec(),
        };
        if write_frame(&mut writer, &chunk).await.is_err() {
            break;
        }
    }
    let _ = write_frame(&mut writer, &ClientFrame::StdinEof).await;
    let _ = writer.shutdown().await;
}

/// Stream the broker's response frames to `stdout` and `stderr` and return the
/// exit code the caller should use. This is the direction that decides the
/// relay's outcome; nothing that happens on the stdin direction changes it.
async fn read_responses<R, O, E>(mut reader: R, stdout: &mut O, stderr: &mut E) -> i32
where
    R: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    E: AsyncWrite + Unpin,
{
    loop {
        match read_frame::<_, ServerFrame>(&mut reader).await {
            Ok(ServerFrame::StdoutChunk { data }) => {
                if stdout.write_all(&data).await.is_err() || stdout.flush().await.is_err() {
                    return GATEWAY_ERROR_EXIT;
                }
            }
            Ok(ServerFrame::StderrChunk { data }) => {
                if stderr.write_all(&data).await.is_err() || stderr.flush().await.is_err() {
                    return GATEWAY_ERROR_EXIT;
                }
            }
            Ok(ServerFrame::CredentialAudit { .. }) => {}
            Ok(ServerFrame::Exit { code }) => {
                let _ = stdout.flush().await;
                let _ = stderr.flush().await;
                return code;
            }
            Ok(ServerFrame::Denied { reason }) => {
                let msg = format!("ghbrk: denied: {reason}\n");
                write_then_flush(stderr, msg.as_bytes()).await;
                return GATEWAY_ERROR_EXIT;
            }
            Err(err) => {
                let msg = match err {
                    ProtocolError::Io(ref io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
                        "ghbrk: broker closed connection before exit\n".to_string()
                    }
                    other => format!("ghbrk: protocol error: {other}\n"),
                };
                write_then_flush(stderr, msg.as_bytes()).await;
                return GATEWAY_ERROR_EXIT;
            }
        }
    }
}

async fn write_then_flush<W>(writer: &mut W, bytes: &[u8])
where
    W: AsyncWrite + Unpin,
{
    let _ = writer.write_all(bytes).await;
    let _ = writer.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::future::Future;
    use std::net::Shutdown;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use tokio::io::ReadBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    /// Payload whose framed encoding is several times the default socket
    /// buffer, so a direction nobody drains blocks instead of disappearing
    /// into the kernel.
    const PAYLOAD_EXCEEDING_SOCKET_BUFFER: usize = 256 * 1024;

    /// Liveness bound: every relay under test either finishes promptly or is
    /// deadlocked, so a generous limit separates the two without racing.
    const RELAY_MUST_FINISH_WITHIN: Duration = Duration::from_secs(10);

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    /// A pipe an unrelated process holds open: it yields no byte and never
    /// reaches end-of-file.
    struct NeverEnds;

    impl AsyncRead for NeverEnds {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
    }

    /// Bind a stub broker at `socket` and serve exactly one connection with
    /// `serve`. Binding happens before the handle is returned, so the relay
    /// under test cannot race the listener.
    fn stub_broker<F, Fut>(socket: &Path, serve: F) -> JoinHandle<Fut::Output>
    where
        F: FnOnce(UnixStream) -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let listener = UnixListener::bind(socket).expect("bind stub broker");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            serve(stream).await
        })
    }

    /// Read client frames until `StdinEof` arrives or the direction ends.
    async fn client_frames(stream: &mut UnixStream) -> Vec<ClientFrame> {
        let mut frames = Vec::new();
        while let Ok(frame) = read_frame::<_, ClientFrame>(stream).await {
            let is_eof = frame == ClientFrame::StdinEof;
            frames.push(frame);
            if is_eof {
                break;
            }
        }
        frames
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

    /// Relay a `gh` invocation, declaring client frames exactly when a source
    /// is supplied, the way `relay` derives the two from one decision.
    async fn relay_gh<S>(
        socket: &Path,
        stdin: Option<S>,
        stdout: &mut Vec<u8>,
        stderr: &mut Vec<u8>,
    ) -> i32
    where
        S: AsyncRead + Unpin,
    {
        let request = Request {
            tool: Tool::Gh,
            args: s(&["pr", "create", "--body-file", "-"]),
            cwd: PathBuf::from("/work/repo"),
            remote_url: None,
            head_branch: None,
            client_frames: stdin.is_some(),
            caller_tmp: None,
        };
        relay_request(request, socket, stdin, stdout, stderr).await
    }

    #[test]
    fn socket_path_defaults_when_env_unset() {
        env::remove_var(SOCKET_ENV_VAR);
        assert_eq!(socket_path_from_env(), PathBuf::from(DEFAULT_SOCKET_PATH));
    }

    #[tokio::test]
    async fn missing_socket_reports_connection_error() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = relay(
            Tool::Git,
            s(&["push", "origin", "main"]),
            dir.path().to_path_buf(),
            &socket,
            None,
            None,
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(code, GATEWAY_ERROR_EXIT);
        let stderr = String::from_utf8(err).unwrap();
        assert!(stderr.contains("cannot connect to broker"), "{stderr}");
        assert!(stderr.contains(&socket.display().to_string()), "{stderr}");
    }

    #[test]
    fn only_child_spawning_tools_forward_stdin() {
        assert!(caller_stdin(Tool::Git, false).is_some());
        assert!(caller_stdin(Tool::Gh, false).is_some());
        assert!(caller_stdin(Tool::Check, false).is_none());
        assert!(caller_stdin(Tool::Explain, false).is_none());
        assert!(caller_stdin(Tool::Policy, false).is_none());
        assert!(caller_stdin(Tool::Allow, false).is_none());
    }

    #[tokio::test]
    async fn piped_stdin_reaches_broker_as_chunks_then_eof() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let request: Request = read_frame(&mut stream).await.expect("request frame");
            let frames = client_frames(&mut stream).await;
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            (request, frames)
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = relay_gh(&socket, Some(&b"body text\n"[..]), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let (request, frames) = server.await.unwrap();
        assert!(request.client_frames);
        assert_eq!(stdin_bytes(&frames), b"body text\n");
        assert_eq!(frames.last(), Some(&ClientFrame::StdinEof));
        assert_eq!(
            frames
                .iter()
                .filter(|frame| **frame == ClientFrame::StdinEof)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn exhausted_stdin_source_sends_only_eof() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let request: Request = read_frame(&mut stream).await.expect("request frame");
            let frames = client_frames(&mut stream).await;
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            (request, frames)
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = relay_gh(&socket, Some(&b""[..]), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let (request, frames) = server.await.unwrap();
        assert!(request.client_frames);
        assert_eq!(frames, vec![ClientFrame::StdinEof]);
    }

    #[tokio::test]
    async fn terminal_stdin_sends_eof_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let _request: Request = read_frame(&mut stream).await.expect("request frame");
            let frames = client_frames(&mut stream).await;
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            frames
        });

        let terminal = caller_stdin(Tool::Gh, true).expect("gh declares client frames");
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = relay_gh(&socket, Some(terminal), &mut out, &mut err).await;

        assert_eq!(code, 0);
        assert_eq!(server.await.unwrap(), vec![ClientFrame::StdinEof]);
    }

    #[tokio::test]
    async fn explain_request_sends_no_client_frames() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let request: Request = read_frame(&mut stream).await.expect("request frame");
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            let trailing = read_frame::<_, ClientFrame>(&mut stream).await;
            (request, trailing.is_err())
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = relay(
            Tool::Explain,
            s(&["git", "push", "origin", "main"]),
            PathBuf::from("/work/repo"),
            &socket,
            None,
            None,
            &mut out,
            &mut err,
        )
        .await;

        assert_eq!(code, 0);
        let (request, direction_carried_nothing_else) = server.await.unwrap();
        assert!(!request.client_frames);
        assert!(
            direction_carried_nothing_else,
            "explain must send the request and nothing else"
        );
    }

    #[tokio::test]
    async fn stdin_chunk_respects_read_bound() {
        let payload: Vec<u8> = (0..STDIN_CHUNK_SIZE * 3 + 100)
            .map(|i| (i % 251) as u8)
            .collect();
        let expected = payload.clone();
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let _request: Request = read_frame(&mut stream).await.expect("request frame");
            let frames = client_frames(&mut stream).await;
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            frames
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = relay_gh(&socket, Some(&payload[..]), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let frames = server.await.unwrap();
        let chunks: Vec<usize> = frames
            .iter()
            .filter_map(|frame| match frame {
                ClientFrame::StdinChunk { data } => Some(data.len()),
                ClientFrame::StdinEof => None,
            })
            .collect();
        assert_eq!(
            chunks,
            vec![STDIN_CHUNK_SIZE, STDIN_CHUNK_SIZE, STDIN_CHUNK_SIZE, 100]
        );
        assert_eq!(stdin_bytes(&frames), expected);
    }

    #[tokio::test]
    async fn stdin_to_non_reading_peer_completes_on_exit() {
        let payload = vec![b'x'; PAYLOAD_EXCEEDING_SOCKET_BUFFER];
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let request: Request = read_frame(&mut stream).await.expect("request frame");
            write_frame(
                &mut stream,
                &ServerFrame::StdoutChunk {
                    data: b"hi".to_vec(),
                },
            )
            .await
            .expect("stdout frame");
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            (request, stream)
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = timeout(
            RELAY_MUST_FINISH_WITHIN,
            relay_gh(&socket, Some(&payload[..]), &mut out, &mut err),
        )
        .await
        .expect("a peer that never drains stdin must not stall the relay");

        assert_eq!(code, 0);
        assert_eq!(out, b"hi");
        let (request, _connection_held_open) = server.await.unwrap();
        assert!(request.client_frames);
    }

    #[tokio::test]
    async fn gateway_reads_output_while_sending_stdin() {
        let payload = vec![b'a'; PAYLOAD_EXCEEDING_SOCKET_BUFFER];
        let expected_stdin = payload.clone();
        let output = vec![b'o'; PAYLOAD_EXCEEDING_SOCKET_BUFFER];
        let expected_output = output.clone();
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, move |mut stream| async move {
            let _request: Request = read_frame(&mut stream).await.expect("request frame");
            // Fill the response direction before draining the stdin direction.
            // Only a gateway that interleaves the two can unblock this write.
            write_frame(&mut stream, &ServerFrame::StdoutChunk { data: output })
                .await
                .expect("stdout frame");
            let frames = client_frames(&mut stream).await;
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            frames
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = timeout(
            RELAY_MUST_FINISH_WITHIN,
            relay_gh(&socket, Some(&payload[..]), &mut out, &mut err),
        )
        .await
        .expect("both directions full at once must not deadlock the relay");

        assert_eq!(code, 0);
        assert_eq!(out, expected_output);
        assert_eq!(stdin_bytes(&server.await.unwrap()), expected_stdin);
    }

    #[tokio::test]
    async fn exit_frame_stops_stdin_forwarding() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let _request: Request = read_frame(&mut stream).await.expect("request frame");
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            stream
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = timeout(
            RELAY_MUST_FINISH_WITHIN,
            relay_gh(&socket, Some(tokio::io::repeat(b'x')), &mut out, &mut err),
        )
        .await
        .expect("an endless pipe must not outlive the exit frame");

        assert_eq!(code, 0);
        let _connection_held_open = server.await.unwrap();
    }

    #[tokio::test]
    async fn open_pipe_sends_no_eof_until_it_closes() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |mut stream| async move {
            let _request: Request = read_frame(&mut stream).await.expect("request frame");
            write_frame(&mut stream, &ServerFrame::Exit { code: 0 })
                .await
                .expect("exit frame");
            read_frame::<_, ClientFrame>(&mut stream).await.is_err()
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = timeout(
            RELAY_MUST_FINISH_WITHIN,
            relay_gh(&socket, Some(NeverEnds), &mut out, &mut err),
        )
        .await
        .expect("a pipe that never closes must not stall the relay");

        assert_eq!(code, 0);
        assert!(
            server.await.unwrap(),
            "no client frame may be sent while the caller's pipe stays open"
        );
    }

    #[tokio::test]
    async fn broken_stdin_pipe_preserves_exit_code() {
        let payload = vec![b'x'; PAYLOAD_EXCEEDING_SOCKET_BUFFER];
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let server = stub_broker(&socket, |stream| async move {
            let mut stream = stream;
            let _request: Request = read_frame(&mut stream).await.expect("request frame");
            // Close only the receiving direction: every further stdin write
            // fails with a broken pipe while the exit frame stays deliverable.
            let closed_for_reading = stream.into_std().expect("into_std");
            closed_for_reading
                .shutdown(Shutdown::Read)
                .expect("shutdown read");
            let mut stream = UnixStream::from_std(closed_for_reading).expect("from_std");
            write_frame(&mut stream, &ServerFrame::Exit { code: 3 })
                .await
                .expect("exit frame");
            stream
        });

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = timeout(
            RELAY_MUST_FINISH_WITHIN,
            relay_gh(&socket, Some(&payload[..]), &mut out, &mut err),
        )
        .await
        .expect("a broken stdin direction must not stall the relay");

        assert_eq!(code, 3);
        assert!(err.is_empty(), "stderr: {}", String::from_utf8_lossy(&err));
        let _connection_held_open = server.await.unwrap();
    }

    #[tokio::test]
    async fn pump_ends_when_the_stdin_direction_breaks() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("broker.sock");
        let listener = UnixListener::bind(&socket).expect("bind stub broker");
        let closed = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            drop(stream);
        });
        let stream = UnixStream::connect(&socket).await.expect("connect");
        closed.await.unwrap();
        let (_read_half, write_half) = stream.into_split();

        timeout(
            RELAY_MUST_FINISH_WITHIN,
            pump_stdin(write_half, tokio::io::repeat(b'x')),
        )
        .await
        .expect("a broken stdin direction must end the pump, not spin on it");
    }
}
