//! Child process executor with real-time stdout/stderr streaming.
//!
//! This module spawns the requested binary (`git`, `gh`, …), with the
//! caller-supplied cwd and the broker-injected env vars, and forwards every
//! chunk read from stdout/stderr back through the wire protocol as
//! `StdoutChunk` / `StderrChunk` frames. The final frame is `Exit { code }`.
//!
//! Memory bound: each read is into a fixed-size 8 KiB buffer; the chunk is
//! immediately serialised into a frame and written. There is no intermediate
//! per-stream accumulator. A 100 MiB stdout produces ~12,800 frames, each
//! holding at most 8 KiB, never accumulating in memory.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::process::Command;

use crate::protocol::{write_frame, ProtocolError, ServerFrame};

/// Size of each stdout/stderr read buffer. Bounded to keep the daemon's
/// resident memory flat regardless of the child's total output volume.
pub const READ_BUF_SIZE: usize = 8 * 1024;

/// Conventional shell encoding for signal-terminated processes: exit code =
/// 128 + signal number.
pub const SIGNAL_EXIT_OFFSET: i32 = 128;

/// Errors raised while spawning or streaming a child.
#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),
}

/// Description of the child process to launch.
#[derive(Debug, Clone)]
pub struct ChildSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// Spawn the child described by `spec` and stream its output to `writer`.
///
/// The contract:
///
/// - Stdin is closed (no inherit, no pipe) so the child cannot block on input.
/// - Stdout and stderr are piped and read concurrently.
/// - For every chunk read from stdout, one `StdoutChunk` frame is written.
/// - For every chunk read from stderr, one `StderrChunk` frame is written.
/// - On clean exit a final `Exit { code }` frame is written.
/// - On spawn failure (e.g. binary not found) a single `Denied { reason }`
///   frame is written and the function returns `Ok(())`. The daemon must NOT
///   crash on spawn failure.
pub async fn stream_child<W>(spec: &ChildSpec, writer: &mut W) -> Result<(), ExecutorError>
where
    W: AsyncWrite + Unpin,
{
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Strip the parent env so secrets we did not whitelist cannot leak.
        .env_clear();
    for (k, v) in &spec.env {
        command.env(k, v);
    }
    // PATH must be present for the kernel to resolve relative program names.
    if !spec.env.iter().any(|(k, _)| k == "PATH") {
        if let Ok(path) = std::env::var("PATH") {
            command.env("PATH", path);
        }
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(err) => {
            let frame = ServerFrame::Denied {
                reason: format!("failed to spawn '{}': {}", spec.program, err),
            };
            write_frame(writer, &frame).await?;
            return Ok(());
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    stream_pipes(stdout, stderr, writer).await?;

    let status = child.wait().await?;
    let code = exit_code_from_status(&status);
    write_frame(writer, &ServerFrame::Exit { code }).await?;
    Ok(())
}

/// Concurrently read from stdout and stderr, emitting one frame per read.
///
/// We use `tokio::select!` over the two readers so the order in which bytes
/// appear at the daemon is preserved on the wire (no merging, no per-stream
/// buffering past one read).
async fn stream_pipes<O, E, W>(
    stdout: Option<O>,
    stderr: Option<E>,
    writer: &mut W,
) -> Result<(), ExecutorError>
where
    O: AsyncRead + Unpin,
    E: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut stdout = stdout;
    let mut stderr = stderr;
    let mut stdout_buf = vec![0u8; READ_BUF_SIZE];
    let mut stderr_buf = vec![0u8; READ_BUF_SIZE];

    loop {
        match (stdout.as_mut(), stderr.as_mut()) {
            (Some(out), Some(err)) => {
                tokio::select! {
                    res = out.read(&mut stdout_buf) => {
                        match res? {
                            0 => { stdout = None; }
                            n => {
                                let frame = ServerFrame::StdoutChunk {
                                    data: stdout_buf[..n].to_vec(),
                                };
                                write_frame(writer, &frame).await?;
                            }
                        }
                    }
                    res = err.read(&mut stderr_buf) => {
                        match res? {
                            0 => { stderr = None; }
                            n => {
                                let frame = ServerFrame::StderrChunk {
                                    data: stderr_buf[..n].to_vec(),
                                };
                                write_frame(writer, &frame).await?;
                            }
                        }
                    }
                }
            }
            (Some(out), None) => match out.read(&mut stdout_buf).await? {
                0 => stdout = None,
                n => {
                    let frame = ServerFrame::StdoutChunk {
                        data: stdout_buf[..n].to_vec(),
                    };
                    write_frame(writer, &frame).await?;
                }
            },
            (None, Some(err)) => match err.read(&mut stderr_buf).await? {
                0 => stderr = None,
                n => {
                    let frame = ServerFrame::StderrChunk {
                        data: stderr_buf[..n].to_vec(),
                    };
                    write_frame(writer, &frame).await?;
                }
            },
            (None, None) => return Ok(()),
        }
    }
}

#[cfg(unix)]
fn exit_code_from_status(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        return code;
    }
    if let Some(sig) = status.signal() {
        return SIGNAL_EXIT_OFFSET + sig;
    }
    -1
}

#[cfg(not(unix))]
fn exit_code_from_status(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// Convenience constructor for callers who already have references into a
/// `Request` and a credential env list.
pub fn spec_from_request(
    program: impl Into<String>,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> ChildSpec {
    ChildSpec {
        program: program.into(),
        args: args.to_vec(),
        env: env.to_vec(),
        cwd: cwd.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::read_frame;
    use std::io::Cursor;

    async fn collect_frames(buf: Vec<u8>) -> Vec<ServerFrame> {
        let mut cursor = Cursor::new(buf);
        let mut out = Vec::new();
        loop {
            match read_frame::<_, ServerFrame>(&mut cursor).await {
                Ok(f) => out.push(f),
                Err(_) => return out,
            }
        }
    }

    #[tokio::test]
    async fn exit_code_zero_on_success() {
        let spec = ChildSpec {
            program: "true".into(),
            args: vec![],
            env: vec![],
            cwd: std::env::current_dir().unwrap(),
        };
        let mut buf = Vec::new();
        stream_child(&spec, &mut buf).await.unwrap();
        let frames = collect_frames(buf).await;
        assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
    }

    #[tokio::test]
    async fn exit_code_nonzero_on_failure() {
        let spec = ChildSpec {
            program: "false".into(),
            args: vec![],
            env: vec![],
            cwd: std::env::current_dir().unwrap(),
        };
        let mut buf = Vec::new();
        stream_child(&spec, &mut buf).await.unwrap();
        let frames = collect_frames(buf).await;
        match frames.last() {
            Some(ServerFrame::Exit { code }) => assert_ne!(*code, 0),
            other => panic!("expected non-zero Exit, got {other:?}"),
        }
    }
}
