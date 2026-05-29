use std::env;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::passthrough::exec_passthrough;
use crate::protocol::{read_frame, write_frame, ProtocolError, Request, ServerFrame, Tool};

/// Default broker socket path, overridable via `GHBRK_SOCKET`.
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/ghbrk/broker.sock";

/// Environment variable that overrides the default broker socket path.
pub const SOCKET_ENV_VAR: &str = "GHBRK_SOCKET";

/// Exit code used when the broker cannot be reached or the protocol fails.
pub const SHIM_ERROR_EXIT: i32 = 1;

/// POSIX EACCES errno value — stable across Linux and macOS.
const EACCES: i32 = 13;

/// Result of a captured shim run for testing.
#[derive(Debug)]
pub struct ShimOutcome {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Resolve the broker socket path, honouring `GHBRK_SOCKET` when set.
pub fn socket_path_from_env() -> PathBuf {
    env::var_os(SOCKET_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH))
}

/// Run the shim against `socket_path`, writing real-time output through the
/// supplied async writers. Returns the exit code the calling process should use.
#[allow(clippy::too_many_arguments)]
pub async fn run_shim_with_io<O, E>(
    tool: Tool,
    args: Vec<String>,
    cwd: PathBuf,
    socket_path: &Path,
    real_path: &str,
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
            // EACCES — POSIX errno, stable across Linux/macOS. The broker
            // socket exists but this process lacks permission to connect, so
            // policy enforcement is provably impossible regardless of what
            // ghbrk does. Silently exec the real binary; never returns.
            if err.raw_os_error() == Some(EACCES) {
                exec_passthrough(real_path, &args);
            }
            let msg = format!(
                "ghbrk: cannot connect to broker at {}: {}\n",
                socket_path.display(),
                err
            );
            let _ = stderr.write_all(msg.as_bytes()).await;
            let _ = stderr.flush().await;
            return SHIM_ERROR_EXIT;
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
        let _ = stderr.write_all(msg.as_bytes()).await;
        let _ = stderr.flush().await;
        return SHIM_ERROR_EXIT;
    }

    let mut reader = read_half;
    loop {
        match read_frame::<_, ServerFrame>(&mut reader).await {
            Ok(ServerFrame::StdoutChunk { data }) => {
                if stdout.write_all(&data).await.is_err() {
                    return SHIM_ERROR_EXIT;
                }
                if stdout.flush().await.is_err() {
                    return SHIM_ERROR_EXIT;
                }
            }
            Ok(ServerFrame::StderrChunk { data }) => {
                if stderr.write_all(&data).await.is_err() {
                    return SHIM_ERROR_EXIT;
                }
                if stderr.flush().await.is_err() {
                    return SHIM_ERROR_EXIT;
                }
            }
            Ok(ServerFrame::Exit { code }) => {
                let _ = stdout.flush().await;
                let _ = stderr.flush().await;
                return code;
            }
            Ok(ServerFrame::Denied { reason }) => {
                let msg = format!("ghbrk: denied: {reason}\n");
                let _ = stderr.write_all(msg.as_bytes()).await;
                let _ = stderr.flush().await;
                return SHIM_ERROR_EXIT;
            }
            Err(err) => {
                let msg = match err {
                    ProtocolError::Io(ref io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
                        "ghbrk: broker closed connection before exit\n".to_string()
                    }
                    other => format!("ghbrk: protocol error: {other}\n"),
                };
                let _ = stderr.write_all(msg.as_bytes()).await;
                let _ = stderr.flush().await;
                return SHIM_ERROR_EXIT;
            }
        }
    }
}

/// Run the shim against the broker socket resolved from the environment, with
/// real stdio attached. Suitable for use from synchronous `main`-side code by
/// constructing a Tokio runtime around it.
pub async fn run_shim(
    tool: Tool,
    args: Vec<String>,
    cwd: PathBuf,
    socket_path: &Path,
    real_path: &str,
    remote_url: Option<String>,
    head_branch: Option<String>,
) -> i32 {
    let mut out = tokio::io::stdout();
    let mut err = tokio::io::stderr();
    run_shim_with_io(
        tool,
        args,
        cwd,
        socket_path,
        real_path,
        remote_url,
        head_branch,
        &mut out,
        &mut err,
    )
    .await
}
