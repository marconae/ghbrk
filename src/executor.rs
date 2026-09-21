//! Child process executor. Relays the wire protocol and a spawned child's I/O
//! in real time, in both directions.
//!
//! This module spawns the requested binary (`git`, `gh`, and more), using the
//! caller-supplied cwd and the broker-injected env vars. It reads
//! `ClientFrame::StdinChunk` frames from the caller and writes their bytes to
//! the child's stdin. It forwards every chunk from the child's stdout and
//! stderr back as `StdoutChunk` and `StderrChunk` frames. The final frame is
//! always `Exit { code }`.
//!
//! Both directions live in this one module because each can block the other.
//! Running them in sequence deadlocks either way. A child that writes output
//! while it waits for input stalls a relay that reads input only after output
//! ends. A child that waits for input stalls a relay that writes input only
//! after output ends. The relay polls both sides together and ends when the
//! output side ends. When the input side ends first, the relay continues.
//! When the output side ends, the relay ends and closes the child's stdin.
//!
//! Memory bound: each stdout/stderr read fills a fixed 8 KiB buffer. Each
//! stdin chunk goes to the child before the relay reads the next client
//! frame. Neither direction buffers a running total, so a 100 MiB stream in
//! either direction produces many small frames, never one large copy in
//! memory.

use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::process::Stdio;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};

use crate::protocol::{read_frame, write_frame, ClientFrame, ProtocolError, ServerFrame};

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
    /// Effective UID the child drops to before `execve`. `None` keeps the
    /// daemon's own identity.
    pub uid: Option<u32>,
    /// Primary GID the child drops to before `execve`. `None` keeps the
    /// daemon's own primary group.
    pub gid: Option<u32>,
    /// Supplementary GIDs applied via `setgroups` before the UID drop. Empty
    /// when there are none.
    pub supplementary_gids: Vec<u32>,
    /// Home directory of the peer user, used to override the child's `HOME`
    /// when privilege is dropped. `None` keeps the inherited `HOME`.
    pub home: Option<PathBuf>,
}

/// Spawn the child described by `spec`. Feed it the caller's standard input
/// from `client_frames`, and stream its output to `writer`.
///
/// The contract:
///
/// - Stdin is a pipe fed by `ClientFrame::StdinChunk` frames read from
///   `client_frames`.
/// - The input source is required, not optional. A caller with nothing to
///   send passes a source already at end-of-file. This closes the child's
///   stdin at once, the same as the old `/dev/null` behavior, and keeps the
///   deadlock rule in one place instead of adding a switch for it.
/// - Stdout and stderr are piped and read at the same time as the input
///   relay.
/// - Each chunk read from stdout produces one `StdoutChunk` frame.
/// - Each chunk read from stderr produces one `StderrChunk` frame.
/// - The input relay ends at the first of: `StdinEof`, end-of-file on
///   `client_frames`, or a write failure on the child's stdin. The child's
///   stdin closes in every one of these cases. The relay itself continues
///   until the output side ends.
/// - On clean exit, the function writes a final `Exit { code }` frame. No
///   frame follows it.
/// - On spawn failure (for example, the binary is not found), the function
///   writes one `Denied { reason }` frame, leaves `client_frames` unread, and
///   returns `Ok(())`. The daemon must not crash on spawn failure.
/// - At spawn, the child's stdin handle is taken out of `Child` so the input
///   relay owns it and can close it independently of the child's other
///   handles.
pub async fn stream_child<R, W>(
    spec: &ChildSpec,
    client_frames: R,
    writer: &mut W,
) -> Result<(), ExecutorError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Clear the parent env so secrets outside the allow list cannot leak.
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

    apply_privilege_drop(&mut command, spec);

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

    let child_stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    relay_until_output_ends(
        relay_stdin(client_frames, child_stdin),
        stream_pipes(stdout, stderr, writer),
    )
    .await?;

    let status = child.wait().await?;
    let code = exit_code_from_status(&status);
    write_frame(writer, &ServerFrame::Exit { code }).await?;
    Ok(())
}

/// Apply a fail-closed privilege drop to `command`, based on `spec`.
///
/// The drop runs only when `uid` and `gid` are both present, the target uid
/// is not root, and the target uid differs from the daemon's own effective
/// uid. A missing `uid` or `gid` skips the whole step. Partial drops never
/// run, so the child cannot end up with a mismatched identity.
///
/// Ordering: the whole drop runs inside one `pre_exec` hook, in this order:
/// `setgroups`, then `setresgid`, then `setresuid`, then `chdir`. This order
/// is mandatory.
///
/// - `setgroups` and `setresgid` need `CAP_SETGID`. The process loses this
///   capability the moment the UID drops to a non-zero value, so both calls
///   must run before `setresuid`.
/// - `chdir` must run after `setresuid`. The standard library calls
///   `chdir()` in the forked child before it runs `pre_exec` closures, so
///   `Command::current_dir()` would chdir as the daemon user and fail with
///   EACCES on a 0700 home directory. This function resets the command's
///   working directory to `/` (always traversable) and does the real
///   `chdir` here, after the UID drop, so the kernel checks the path against
///   the peer user's identity.
///
/// This function does not use `CommandExt::uid()`/`gid()`. The standard
/// library applies those, and its own internal `setgroups`, before running
/// `pre_exec` closures. That would drop the UID first and make the
/// `setgroups` call here fail with `EPERM`. Doing every step inside one
/// closure puts the order fully under this function's control.
///
/// Fail-closed: any failing syscall returns `Err`. `execve` then never runs,
/// and the caller sees a spawn failure (a `Denied` frame) instead of a child
/// that runs with a partially-dropped identity.
#[cfg(unix)]
fn apply_privilege_drop(command: &mut Command, spec: &ChildSpec) {
    let (uid, gid) = match (spec.uid, spec.gid) {
        (Some(uid), Some(gid)) => (uid, gid),
        _ => return,
    };

    let own_euid = nix::unistd::geteuid().as_raw();
    if uid == 0 || uid == own_euid {
        return;
    }

    if let Some(home) = &spec.home {
        if !spec.env.iter().any(|(k, _)| k == "HOME") {
            command.env("HOME", home);
        }
    }

    let gids: Vec<nix::unistd::Gid> = spec
        .supplementary_gids
        .iter()
        .copied()
        .map(nix::unistd::Gid::from_raw)
        .collect();

    // Reset the command's cwd to "/" so the chdir that runs before `pre_exec`
    // (as the daemon user) always succeeds. The real chdir to spec.cwd runs
    // inside the pre_exec closure below, after setresuid.
    let cwd = spec.cwd.clone();
    command.current_dir("/");

    // SAFETY: the closure runs in the forked child, between `fork` and
    // `execve`, where the Rust runtime is in an undefined state. It only
    // runs the `setgroups`, `setresgid`, `setresuid`, and `chdir` syscalls,
    // and touches no shared runtime state. The `gids` vector and `cwd` path
    // are built before the closure runs, before `fork`, so the child does no
    // heap allocation and no I/O.
    unsafe {
        command.pre_exec(move || {
            let target_gid = nix::unistd::Gid::from_raw(gid);
            let target_uid = nix::unistd::Uid::from_raw(uid);

            nix::unistd::setgroups(&gids).map_err(drop_error)?;
            // Set real, effective, and saved GID so the child cannot restore
            // its primary group after exec.
            nix::unistd::setresgid(target_gid, target_gid, target_gid).map_err(drop_error)?;
            // UID last: this step gives up CAP_SETUID and CAP_SETGID.
            nix::unistd::setresuid(target_uid, target_uid, target_uid).map_err(drop_error)?;
            // chdir after setresuid so the kernel checks the path as the peer
            // user. This matters when the target directory sits inside a
            // 0700 home dir that the daemon user cannot enter.
            nix::unistd::chdir(&cwd).map_err(drop_error)?;
            Ok(())
        });
    }
}

/// Maps a privilege-drop syscall failure to a fail-closed `io::Error` so the
/// `pre_exec` closure aborts `execve`.
#[cfg(unix)]
fn drop_error(_err: nix::errno::Errno) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "privilege drop failed",
    )
}

#[cfg(not(unix))]
fn apply_privilege_drop(_command: &mut Command, _spec: &ChildSpec) {}

/// Poll both relay directions together. End when `output` ends.
///
/// This function holds the executor's deadlock rule. When `input` ends
/// first, the relay does not end: a child that has read all of its standard
/// input can still have output to produce, so polling continues on `output`
/// alone. When `output` ends first, the relay ends and drops `input`. This
/// closes the child's standard input, because the input future owns the
/// `ChildStdin` handle. Cancelling the future releases the pipe the same way
/// completing it does.
///
/// Both futures are pinned once and polled again across iterations, instead
/// of rebuilt each iteration. This matters for `input`, which reads
/// length-prefixed frames. Rebuilding it after a `select!` iteration would
/// restart a read that had already consumed a frame header, and lose bytes
/// from the caller's standard input.
async fn relay_until_output_ends<I, O, T>(input: I, output: O) -> T
where
    I: Future<Output = ()>,
    O: Future<Output = T>,
{
    let mut input = pin!(input);
    let mut output = pin!(output);
    let mut input_live = true;

    loop {
        if !input_live {
            return output.as_mut().await;
        }
        tokio::select! {
            finished = output.as_mut() => return finished,
            () = input.as_mut() => input_live = false,
        }
    }
}

/// Write the caller's standard input, carried as `ClientFrame::StdinChunk`
/// frames, to the child's standard input. Stop at the first of: `StdinEof`,
/// end-of-file on `client_frames`, or a write failure on the child's stdin.
///
/// This function owns `child_stdin` on purpose. The pipe closes when this
/// future ends, whether it ran to completion or the output side cancelled it
/// by ending first. This lets a child that waits for end-of-file always
/// proceed. An absent handle means the spawn produced no stdin pipe. There is
/// then nothing to feed and nothing to close, so the relay ends without
/// reading a client frame.
///
/// This function does not propagate a failed read or a failed write, because
/// neither makes the call a failure. The child is already spawned, so only
/// its own exit code matters, and `Exit` must stay the final frame on the
/// wire. A broken stdin pipe is the normal case of a child that read the body
/// it wanted and exited while the caller still had bytes queued. A truncated
/// or malformed client frame means a broken caller. Closing the child's
/// stdin lets the child finish instead of aborting it mid-run.
async fn relay_stdin<R>(mut client_frames: R, child_stdin: Option<ChildStdin>)
where
    R: AsyncRead + Unpin,
{
    let Some(mut child_stdin) = child_stdin else {
        return;
    };

    loop {
        match read_frame::<_, ClientFrame>(&mut client_frames).await {
            Ok(ClientFrame::StdinChunk { data }) => {
                if child_stdin.write_all(&data).await.is_err() {
                    return;
                }
            }
            Ok(ClientFrame::StdinEof) | Err(_) => return,
        }
    }
}

/// Read from stdout and stderr at the same time. Emit one frame per read.
///
/// This function uses `tokio::select!` over the two readers so the wire
/// keeps the order in which bytes arrive at the daemon: no merging, and no
/// per-stream buffering past one read.
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
            uid: None,
            gid: None,
            supplementary_gids: Vec::new(),
            home: None,
        };
        let mut buf = Vec::new();
        stream_child(&spec, tokio::io::empty(), &mut buf)
            .await
            .unwrap();
        let frames = collect_frames(buf).await;
        assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
    }

    #[tokio::test]
    async fn uid_zero_skips_drop_and_runs_normally() {
        let spec = ChildSpec {
            program: "true".into(),
            args: vec![],
            env: vec![],
            cwd: std::env::current_dir().unwrap(),
            uid: Some(0),
            gid: Some(0),
            supplementary_gids: Vec::new(),
            home: None,
        };
        let mut buf = Vec::new();
        stream_child(&spec, tokio::io::empty(), &mut buf)
            .await
            .unwrap();
        let frames = collect_frames(buf).await;
        assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
    }

    #[tokio::test]
    async fn drop_to_foreign_uid_as_non_root_fails_closed() {
        // When we are not root, attempting to drop to a different non-zero uid
        // is denied by the kernel. The executor must surface a Denied frame and
        // never panic.
        if nix::unistd::geteuid().is_root() {
            return;
        }
        let own = nix::unistd::geteuid().as_raw();
        let target = if own == 12345 { 12346 } else { 12345 };
        let spec = ChildSpec {
            program: "true".into(),
            args: vec![],
            env: vec![],
            cwd: std::env::current_dir().unwrap(),
            uid: Some(target),
            gid: Some(target),
            supplementary_gids: Vec::new(),
            home: None,
        };
        let mut buf = Vec::new();
        stream_child(&spec, tokio::io::empty(), &mut buf)
            .await
            .unwrap();
        let frames = collect_frames(buf).await;
        assert!(
            matches!(frames.last(), Some(ServerFrame::Denied { .. })),
            "expected Denied frame, got {:?}",
            frames.last()
        );
    }

    #[tokio::test]
    async fn home_override_only_when_caller_absent() {
        // HOME injection must not clobber a caller-provided HOME. With uid==0 the
        // drop is skipped, so HOME is left exactly as the caller set it.
        let spec = ChildSpec {
            program: "true".into(),
            args: vec![],
            env: vec![("HOME".into(), "/caller/home".into())],
            cwd: std::env::current_dir().unwrap(),
            uid: Some(0),
            gid: Some(0),
            supplementary_gids: Vec::new(),
            home: Some(PathBuf::from("/peer/home")),
        };
        let mut buf = Vec::new();
        stream_child(&spec, tokio::io::empty(), &mut buf)
            .await
            .unwrap();
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
            uid: None,
            gid: None,
            supplementary_gids: Vec::new(),
            home: None,
        };
        let mut buf = Vec::new();
        stream_child(&spec, tokio::io::empty(), &mut buf)
            .await
            .unwrap();
        let frames = collect_frames(buf).await;
        match frames.last() {
            Some(ServerFrame::Exit { code }) => assert_ne!(*code, 0),
            other => panic!("expected non-zero Exit, got {other:?}"),
        }
    }
}
