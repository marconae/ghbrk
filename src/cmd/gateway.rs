//! Broker-relay transport for the explicit `ghbrk git` / `ghbrk gh` gateways.
//!
//! Connects directly to the broker socket, writes a single `Request` frame,
//! and streams the broker's `ServerFrame` responses to the process's real
//! stdio. Unlike the former transparent shim, there is no config, no
//! passthrough exec, and no EACCES silent fall-through: if the broker cannot
//! be reached, the gateway reports the failure and exits non-zero.

use std::env;
use std::path::{Path, PathBuf};
use std::process;

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use ghbrk::protocol::{read_frame, write_frame, ProtocolError, Request, ServerFrame, Tool};

/// Default broker socket path, overridable via `GHBRK_SOCKET`.
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/ghbrk/broker.sock";

/// Environment variable that overrides the default broker socket path.
pub const SOCKET_ENV_VAR: &str = "GHBRK_SOCKET";

/// Exit code used when the broker cannot be reached or the protocol fails.
pub const GATEWAY_ERROR_EXIT: i32 = 1;

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

/// Core async relay loop, generic over the stdio writers so it can be tested
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
    let request = Request {
        tool,
        args,
        cwd,
        remote_url,
        head_branch,
    };

    if let Err(err) = write_frame(&mut write_half, &request).await {
        let msg = format!("ghbrk: failed to send request to broker: {err}\n");
        write_then_flush(stderr, msg.as_bytes()).await;
        return GATEWAY_ERROR_EXIT;
    }

    let mut reader = read_half;
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

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
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
}
