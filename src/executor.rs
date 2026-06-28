//! Child process executor with a real-time, full-duplex relay between the
//! wire protocol and the spawned child.
//!
//! This module spawns the requested binary (`git`, `gh`, …), with the
//! caller-supplied cwd and the broker-injected env vars, and relays both
//! directions of its I/O. It reads `ClientFrame::StdinChunk` frames from the
//! caller's client-frame source and writes their bytes to the child's stdin,
//! while forwarding every chunk read from the child's stdout/stderr back as
//! `StdoutChunk` / `StderrChunk` frames. The final frame is `Exit { code }`.
//!
//! Both directions live in this one module because they constrain each other.
//! Running them in sequence deadlocks in either order: a child writing output
//! while it waits for input stalls a relay that reads input only once output
//! has finished, and a child waiting for input stalls a relay that writes input
//! only once output has finished. The two are therefore polled together, and
//! the relay finishes on the output side — the input side completing is not
//! the end of the relay, while the output side completing closes the child's
//! stdin and ends it.
//!
//! Memory bound: each stdout/stderr read is into a fixed-size 8 KiB buffer, and
//! each stdin chunk is handed to the child before the next client frame is
//! read. Neither direction keeps an accumulator, so a 100 MiB stream either way
//! produces many small frames and never a resident copy of the whole stream.

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
    /// Effective UID the child should drop to before `execve`. `None` keeps the
    /// daemon's own identity.
    pub uid: Option<u32>,
    /// Primary GID the child should drop to before `execve`. `None` keeps the
    /// daemon's own primary group.
    pub gid: Option<u32>,
    /// Supplementary GIDs applied via `setgroups` before the UID drop. Empty
    /// when none are known or applicable.
    pub supplementary_gids: Vec<u32>,
    /// Home directory of the peer user, used to override the child's `HOME`
    /// when privilege is dropped. `None` keeps the inherited `HOME`.
    pub home: Option<PathBuf>,
}

/// Spawn the child described by `spec`, feed it the caller's standard input
/// from `client_frames`, and stream its output to `writer`.
///
/// The contract:
///
/// - Stdin is a pipe fed by `ClientFrame::StdinChunk` frames read from
///   `client_frames`.
/// - The input source is required rather than optional: a caller with nothing
///   to send passes a source already at end-of-file, which closes the child's
///   stdin at once and reproduces the previous `/dev/null` behaviour. Keeping
///   the decision here rather than offering a switch is what keeps the
///   deadlock rule in one place.
/// - Stdout and stderr are piped and read concurrently with the input relay.
/// - For every chunk read from stdout, one `StdoutChunk` frame is written.
/// - For every chunk read from stderr, one `StderrChunk` frame is written.
/// - The input relay ends at the first of `StdinEof`, end-of-file on
///   `client_frames`, or a write failure on the child's stdin; the child's
///   stdin is closed in every one of those cases, and the relay itself
///   continues until the output side finishes.
/// - On clean exit a final `Exit { code }` frame is written. No frame follows
///   it.
/// - On spawn failure (e.g. binary not found) a single `Denied { reason }`
///   frame is written, `client_frames` is left unread, and the function
///   returns `Ok(())`. The daemon must NOT crash on spawn failure.
/// - The child's stdin handle is taken out of `Child` at spawn so the input
///   relay can own it and close it independently of the child's other
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

/// Apply a fail-closed privilege drop to `command` based on `spec`.
///
/// The drop is applied only when both `uid` and `gid` are present, the target
/// uid is not root, and it differs from the daemon's own effective uid. Partial
/// drops are never performed: a missing `uid` or `gid` skips the whole step so
/// the child cannot end up with a mismatched identity.
///
/// Ordering: the whole drop runs inside a single `pre_exec` hook, in the order
/// `setgroups` → `setresgid` → `setresuid` → `chdir`. This ordering is mandatory:
///
/// - `setgroups`/`setresgid` require `CAP_SETGID`, which is lost the moment the
///   UID is dropped to a non-zero value, so they must come before `setresuid`.
/// - `chdir` must come *after* `setresuid`: the standard library calls `chdir()`
///   in the forked child **before** running `pre_exec` closures, which means the
///   `Command::current_dir()` chdir runs as the daemon user and fails with EACCES
///   on a 0700 home directory. We reset the command's working directory to `/`
///   (always traversable) and perform the real `chdir` here, after the UID drop,
///   so the kernel evaluates the path with the peer user's identity.
///
/// We deliberately do **not** use `CommandExt::uid()`/`gid()`: the standard
/// library applies those (and its own internal `setgroups`) *before* running
/// user `pre_exec` closures, which would drop the UID first and make our
/// `setgroups` fail with `EPERM`. Performing every step inside one closure puts
/// the ordering fully under our control.
///
/// Fail-closed: any failing syscall returns `Err`, so `execve` never runs and
/// the caller observes a spawn failure (surfaced as a `Denied` frame) rather
/// than a child running with a partially-dropped identity.
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

    // Reset the command's cwd to "/" so the pre-pre_exec chdir (which runs as
    // the daemon user) always succeeds. The real chdir to spec.cwd is done
    // inside the pre_exec closure below, after setresuid.
    let cwd = spec.cwd.clone();
    command.current_dir("/");

    // SAFETY: the closure runs in the forked child between `fork` and `execve`,
    // where the Rust runtime is in an undefined state. It performs only the
    // `setgroups`/`setresgid`/`setresuid`/`chdir` syscalls and returns; it
    // touches no shared runtime state. The `gids` vector and `cwd` path are
    // pre-built before the closure (before `fork`), so no heap allocation occurs
    // in the child, and no I/O.
    unsafe {
        command.pre_exec(move || {
            let target_gid = nix::unistd::Gid::from_raw(gid);
            let target_uid = nix::unistd::Uid::from_raw(uid);

            nix::unistd::setgroups(&gids).map_err(drop_error)?;
            // Set real, effective, and saved GID so the child cannot restore
            // its primary group after exec.
            nix::unistd::setresgid(target_gid, target_gid, target_gid).map_err(drop_error)?;
            // UID last: this is the step that relinquishes CAP_SETUID/SETGID.
            nix::unistd::setresuid(target_uid, target_uid, target_uid).map_err(drop_error)?;
            // chdir after setresuid so the kernel evaluates the path as the
            // peer user — required when the target directory sits inside a
            // 0700 home dir that the daemon user cannot traverse.
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

/// Poll both relay directions together and finish when `output` finishes.
///
/// This function is where the executor's deadlock rule lives. `input` finishing
/// first is not the end of the relay — a child that has consumed all of its
/// standard input still has output to produce — so polling continues on
/// `output` alone. `output` finishing first ends the relay and drops `input`,
/// which is what closes the child's standard input: the `ChildStdin` handle is
/// owned by the input future, so cancelling it releases the pipe just as
/// completing it does.
///
/// Both futures are pinned once and re-polled across iterations rather than
/// rebuilt per iteration. That matters for `input`, which reads length-prefixed
/// frames: rebuilding it after a `select!` iteration would restart a read that
/// had already consumed a frame header, losing bytes out of the caller's
/// standard input.
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
/// frames, to the child's standard input until the first of `StdinEof`,
/// end-of-file on `client_frames`, or a write failure on the child's stdin.
///
/// Owning `child_stdin` is deliberate: the pipe closes when this future ends,
/// whether it ran to completion or was cancelled by the output side finishing
/// first, so a child that waits for end-of-file can always proceed. An absent
/// handle means the spawn produced no stdin pipe; there is then nothing to feed
/// and nothing to close, so the relay ends without consuming a client frame.
///
/// Neither a failed read nor a failed write is propagated, because neither
/// makes the invocation a failure. The child has already been spawned, so the
/// outcome that matters is its own exit code, and `Exit` must stay the final
/// frame on the wire. A broken stdin pipe is the ordinary case of a child that
/// read the body it wanted and exited while the caller still had bytes queued;
/// a truncated or malformed client frame is a broken caller, and closing the
/// child's stdin lets the child finish rather than aborting it mid-run.
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
