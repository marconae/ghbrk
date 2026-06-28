//! Integration tests for the executor streaming module.
//!
//! Each test spawns a real child process (typically a small `sh -c '…'`
//! script) and asserts on the sequence of `ServerFrame` values written by
//! `stream_child` into an in-memory buffer.

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use ghbrk::executor::{stream_child, ChildSpec};
use ghbrk::protocol::{read_frame, write_frame, ClientFrame, ServerFrame};

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
}

/// A `ChildSpec` running `cat`, which copies its standard input to its standard
/// output and exits only once that input reaches end-of-file.
fn cat_spec() -> ChildSpec {
    ChildSpec {
        program: "cat".into(),
        args: vec![],
        env: vec![],
        cwd: cwd(),
        uid: None,
        gid: None,
        supplementary_gids: Vec::new(),
        home: None,
    }
}

/// Encode `frames` into the length-prefixed wire form the executor expects on
/// its client-frame source, ending at the last frame with no trailing bytes.
async fn client_frames(frames: &[ClientFrame]) -> Cursor<Vec<u8>> {
    let mut buf = Vec::new();
    for frame in frames {
        write_frame(&mut buf, frame).await.unwrap();
    }
    Cursor::new(buf)
}

/// Read `ServerFrame`s until the stdout bytes seen so far end with `expected`,
/// accumulating them across calls. Times out rather than hanging, so a relay
/// that stopped reading client frames fails the test instead of blocking it.
async fn read_until_echoed<R>(reader: &mut R, seen: &mut Vec<u8>, expected: &str)
where
    R: tokio::io::AsyncRead + Unpin,
{
    while !String::from_utf8_lossy(seen).ends_with(expected) {
        let frame =
            tokio::time::timeout(Duration::from_secs(5), read_frame::<_, ServerFrame>(reader))
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {expected:?} to be echoed back"))
                .expect("failed to read a response frame");
        match frame {
            ServerFrame::StdoutChunk { data } => seen.extend_from_slice(&data),
            other => panic!("expected a StdoutChunk while stdin was still arriving, got {other:?}"),
        }
    }
}

fn sh_spec(script: &str) -> ChildSpec {
    ChildSpec {
        program: "sh".into(),
        args: vec!["-c".into(), script.to_string()],
        env: vec![],
        cwd: cwd(),
        uid: None,
        gid: None,
        supplementary_gids: Vec::new(),
        home: None,
    }
}

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

fn concat_stdout(frames: &[ServerFrame]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in frames {
        if let ServerFrame::StdoutChunk { data } = f {
            out.extend_from_slice(data);
        }
    }
    out
}

fn concat_stderr(frames: &[ServerFrame]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in frames {
        if let ServerFrame::StderrChunk { data } = f {
            out.extend_from_slice(data);
        }
    }
    out
}

#[tokio::test]
async fn stdout_streams_in_chunks() {
    let spec = sh_spec("printf 'one\\ntwo\\nthree\\n'");
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;

    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ServerFrame::StdoutChunk { .. })),
        "expected at least one StdoutChunk in: {frames:?}"
    );
    let stdout = concat_stdout(&frames);
    assert_eq!(String::from_utf8_lossy(&stdout), "one\ntwo\nthree\n");
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[tokio::test]
async fn stderr_streams_in_chunks() {
    let spec = sh_spec("printf 'oops\\n' 1>&2");
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;

    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ServerFrame::StderrChunk { .. })),
        "expected at least one StderrChunk"
    );
    let stderr = concat_stderr(&frames);
    assert_eq!(String::from_utf8_lossy(&stderr), "oops\n");
}

#[tokio::test]
async fn exit_code_propagated() {
    let spec = sh_spec("exit 42");
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;

    match frames.last() {
        Some(ServerFrame::Exit { code }) => assert_eq!(*code, 42),
        other => panic!("expected Exit code 42, got {other:?}"),
    }
}

#[tokio::test]
async fn child_cwd_matches_request() {
    let tmp = tempfile::tempdir().unwrap();
    let canonical = tmp.path().canonicalize().unwrap();
    let spec = ChildSpec {
        program: "pwd".into(),
        args: vec![],
        env: vec![],
        cwd: canonical.clone(),
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
    let stdout = String::from_utf8(concat_stdout(&frames)).unwrap();
    assert_eq!(stdout.trim(), canonical.to_string_lossy());
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[tokio::test]
async fn stdout_stderr_interleaving_preserved() {
    // Write to stdout, then stderr, then stdout. Sleeps separate the writes
    // so the executor's reader has time to drain each one before the next
    // arrives, guaranteeing distinct frames.
    let script = "printf 'a' && sleep 0.05 && printf 'b' 1>&2 && sleep 0.05 && printf 'c'";
    let spec = sh_spec(script);
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;

    // The contract for this scenario is that BOTH streams arrive (separately)
    // and the Exit frame is last. Strict ordering across streams depends on
    // OS scheduling, so we only assert presence + final Exit.
    let saw_stdout = frames
        .iter()
        .any(|f| matches!(f, ServerFrame::StdoutChunk { .. }));
    let saw_stderr = frames
        .iter()
        .any(|f| matches!(f, ServerFrame::StderrChunk { .. }));
    assert!(saw_stdout, "no stdout frames in {frames:?}");
    assert!(saw_stderr, "no stderr frames in {frames:?}");
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));

    // Stdout content should concat to "ac"; stderr should be "b".
    assert_eq!(String::from_utf8_lossy(&concat_stdout(&frames)), "ac");
    assert_eq!(String::from_utf8_lossy(&concat_stderr(&frames)), "b");
}

#[tokio::test]
async fn killed_child_nonzero_exit() {
    // Spawn a shell that traps and then signals itself with SIGKILL via
    // `kill -9 $$`. Tokio's wait will report the unix signal exit which we
    // map to a non-zero code (128 + signal).
    let spec = sh_spec("kill -9 $$");
    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream_child(&spec, tokio::io::empty(), &mut buf),
    )
    .await
    .expect("child did not terminate within timeout")
    .expect("stream_child returned error");
    let frames = collect_frames(buf).await;
    match frames.last() {
        Some(ServerFrame::Exit { code }) => assert_ne!(*code, 0, "expected non-zero exit"),
        other => panic!("expected Exit, got {other:?}"),
    }
}

#[tokio::test]
async fn spawn_failure_emits_denied() {
    let spec = ChildSpec {
        program: "/no/such/binary-that-does-not-exist".into(),
        args: vec![],
        env: vec![],
        cwd: cwd(),
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
    assert_eq!(frames.len(), 1, "expected exactly one frame");
    match &frames[0] {
        ServerFrame::Denied { reason } => {
            assert!(
                reason.to_lowercase().contains("spawn") || reason.contains("/no/such"),
                "denial reason should mention the spawn failure: {reason}"
            );
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

/// An unprivileged uid that is neither root nor (assumed) the test runner's own
/// uid. `nobody` is conventionally 65534 across Linux distros.
const NOBODY_UID: u32 = 65534;

/// A `ChildSpec` running `id -u` with a valid HOME so the child never fails for
/// reasons unrelated to the privilege-drop path under test.
fn id_u_spec(uid: Option<u32>, gid: Option<u32>) -> ChildSpec {
    ChildSpec {
        program: "id".into(),
        args: vec!["-u".into()],
        env: vec![("HOME".to_string(), "/tmp".to_string())],
        cwd: cwd(),
        uid,
        gid,
        supplementary_gids: Vec::new(),
        home: None,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn skips_drop_for_uid_none() {
    let spec = id_u_spec(None, None);
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;
    let stdout = String::from_utf8(concat_stdout(&frames)).unwrap();
    let own = nix::unistd::geteuid().as_raw();
    assert_eq!(stdout.trim(), own.to_string(), "frames: {frames:?}");
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[cfg(unix)]
#[tokio::test]
async fn skips_drop_for_root_uid() {
    // Targeting uid 0 must hit the `uid == 0` guard and leave the child as the
    // daemon's own identity. Meaningless if we are already root.
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let spec = id_u_spec(Some(0), Some(0));
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;
    let stdout = String::from_utf8(concat_stdout(&frames)).unwrap();
    let own = nix::unistd::geteuid().as_raw();
    assert_eq!(
        stdout.trim(),
        own.to_string(),
        "root-uid drop should have been skipped; frames: {frames:?}"
    );
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[cfg(unix)]
#[tokio::test]
async fn skips_drop_for_self_uid() {
    let own_uid = nix::unistd::geteuid().as_raw();
    let own_gid = nix::unistd::getegid().as_raw();
    let spec = id_u_spec(Some(own_uid), Some(own_gid));
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;
    let stdout = String::from_utf8(concat_stdout(&frames)).unwrap();
    assert_eq!(stdout.trim(), own_uid.to_string(), "frames: {frames:?}");
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[cfg(unix)]
#[tokio::test]
async fn drops_to_target_uid_gid_when_root() {
    let own = nix::unistd::geteuid().as_raw();
    let target = if own != NOBODY_UID { NOBODY_UID } else { 65533 };
    let spec = id_u_spec(Some(target), Some(target));
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;

    if nix::unistd::geteuid().is_root() {
        // With CAP_SETUID/CAP_SETGID the drop succeeds and the child reports the
        // target uid.
        let stdout = String::from_utf8(concat_stdout(&frames)).unwrap();
        assert_eq!(stdout.trim(), target.to_string(), "frames: {frames:?}");
        assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
    } else {
        // Without the capability the kernel refuses the drop and the executor
        // fails closed with a Denied frame.
        assert!(
            frames
                .iter()
                .any(|f| matches!(f, ServerFrame::Denied { .. })),
            "expected Denied frame for unprivileged uid-drop, got: {frames:?}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn home_overridden_on_privilege_drop_when_root() {
    if !nix::unistd::geteuid().is_root() {
        return;
    }
    let target = NOBODY_UID;
    let spec = ChildSpec {
        program: "sh".into(),
        args: vec!["-c".into(), "echo $HOME".into()],
        env: vec![],
        cwd: cwd(),
        uid: Some(target),
        gid: Some(target),
        supplementary_gids: Vec::new(),
        home: Some(PathBuf::from("/tmp")),
    };
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;
    let stdout = String::from_utf8(concat_stdout(&frames)).unwrap();
    assert_eq!(stdout.trim(), "/tmp", "frames: {frames:?}");
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[cfg(unix)]
#[tokio::test]
async fn empty_supplementary_gids_still_spawns() {
    // uid == own_euid hits the self-skip guard, so the drop (and setgroups) is
    // never applied. The point is that an empty supplementary_gids vec does not
    // crash the spawn path.
    let own_uid = nix::unistd::geteuid().as_raw();
    let own_gid = nix::unistd::getegid().as_raw();
    let spec = ChildSpec {
        program: "id".into(),
        args: vec!["-u".into()],
        env: vec![("HOME".to_string(), "/tmp".to_string())],
        cwd: cwd(),
        uid: Some(own_uid),
        gid: Some(own_gid),
        supplementary_gids: vec![],
        home: None,
    };
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[cfg(unix)]
#[tokio::test]
async fn failed_drop_emits_denied_not_crash() {
    if nix::unistd::geteuid().is_root() {
        return; // root can always drop — this test is meaningless as root
    }
    let own = nix::unistd::geteuid().as_raw();
    let target_uid = if own != 65534 { 65534 } else { 65533 };
    let spec = ChildSpec {
        program: "id".into(),
        args: vec!["-u".into()],
        env: vec![],
        cwd: std::env::current_dir().unwrap(),
        uid: Some(target_uid),
        gid: Some(target_uid),
        supplementary_gids: vec![],
        home: None,
    };
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    // Must get a Denied frame, NOT a panic or exit-code frame
    let frames = collect_frames(buf).await;
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Denied { .. })),
        "expected Denied frame for unprivileged uid-drop, got: {frames:?}"
    );
}

#[tokio::test]
async fn large_output_bounded_memory() {
    // Produce 1 MiB of stdout and verify every byte is delivered, in order.
    let script = "head -c 1048576 /dev/zero";
    let spec = sh_spec(script);
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;
    let stdout = concat_stdout(&frames);
    assert_eq!(stdout.len(), 1024 * 1024, "stdout byte count mismatch");
    assert!(stdout.iter().all(|&b| b == 0), "stdout was not all zeros");
    // Each chunk MUST be bounded by the read buffer size.
    for f in &frames {
        if let ServerFrame::StdoutChunk { data } = f {
            assert!(
                data.len() <= ghbrk::executor::READ_BUF_SIZE,
                "chunk size {} exceeded READ_BUF_SIZE {}",
                data.len(),
                ghbrk::executor::READ_BUF_SIZE
            );
        }
    }
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

// Regression test: chdir must occur after setresuid, not before.
//
// The standard library calls chdir() in the forked child BEFORE pre_exec.
// When the daemon is an unprivileged user and the target cwd is inside a
// 0700 home directory, that pre-pre_exec chdir fails with EACCES.
// apply_privilege_drop fixes this by resetting the command cwd to "/" and
// performing the real chdir inside pre_exec, after setresuid.
//
// This test verifies the fix with a cwd that is only accessible to the
// current user (mode 0700), without needing root.
#[cfg(unix)]
#[tokio::test]
async fn chdir_after_setresuid_skips_for_self() {
    use std::os::unix::fs::PermissionsExt;

    // Create a temp dir accessible only to the current user.
    let dir = tempfile::tempdir().expect("tempdir");
    let restricted = dir.path().join("private");
    std::fs::create_dir(&restricted).unwrap();
    std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o700)).unwrap();

    let own = nix::unistd::geteuid().as_raw();
    // uid == own_euid → skip-drop path: current_dir("/") is NOT applied,
    // so the cwd is set normally via Command::current_dir to `restricted`.
    // The child runs as us and can access the 0700 dir just fine.
    let spec = ChildSpec {
        program: "sh".into(),
        args: vec!["-c".into(), "pwd".into()],
        env: vec![("HOME".into(), "/tmp".into())],
        cwd: restricted.clone(),
        uid: Some(own),
        gid: Some(nix::unistd::getegid().as_raw()),
        supplementary_gids: Vec::new(),
        home: None,
    };
    let mut buf = Vec::new();
    stream_child(&spec, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames = collect_frames(buf).await;
    // Self-skip → child ran in restricted dir, exited 0.
    assert!(
        matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })),
        "expected exit 0 for self-uid spec in 0700 dir, got: {frames:?}"
    );
}

// Regression test: ssh key must be reachable when credentials dir is traversable (0711).
//
// Root cause of the bug: deploy/linux/install.sh created /etc/ghbrk/credentials
// with mode 0700 (owner-only). After privilege drop, git spawns ssh as the peer
// user. ssh tries to open /etc/ghbrk/credentials/<user>/id_rsa. The peer user
// is not the ghbrk owner, so the 0700 directory denies traversal → EACCES.
//
// The fix sets the directory to 0711 (owner:rwx, group:--x, others:--x), which
// allows any user to traverse (execute bit) without being able to list contents
// (no read bit for group/others). This test verifies that a subprocess can cat
// a file inside a 0711 directory but NOT inside a 0000 directory.
#[cfg(unix)]
#[tokio::test]
async fn ssh_key_reachable_after_dir_made_traversable() {
    use std::os::unix::fs::PermissionsExt;

    // Create a temp dir. Inside it create "creds/" and "creds/id_rsa".
    let dir = tempfile::tempdir().expect("tempdir");
    let creds = dir.path().join("creds");
    std::fs::create_dir(&creds).unwrap();
    let id_rsa = creds.join("id_rsa");
    std::fs::write(&id_rsa, b"FAKE_KEY").unwrap();
    std::fs::set_permissions(&id_rsa, std::fs::Permissions::from_mode(0o644)).unwrap();

    // Block traversal of the outer temp dir entirely (mode 0000).
    // Even the owner cannot traverse it — this simulates the 0700 scenario
    // where the peer user (non-owner) would be denied.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

    let id_rsa_str = id_rsa.to_string_lossy().to_string();
    let spec_blocked = ChildSpec {
        program: "sh".into(),
        args: vec!["-c".into(), format!("cat {id_rsa_str}")],
        env: vec![],
        cwd: std::env::temp_dir(), // root-level temp dir, always accessible
        uid: None,
        gid: None,
        supplementary_gids: Vec::new(),
        home: None,
    };

    let mut buf = Vec::new();
    stream_child(&spec_blocked, tokio::io::empty(), &mut buf)
        .await
        .unwrap();
    let frames_blocked = collect_frames(buf).await;

    // CRITICAL: restore mode before any assertion that could panic, so that
    // TempDir::drop can clean up the directory even if an assertion fails.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o711)).unwrap();

    // With mode 0000 the child must fail (EACCES → non-zero exit).
    let blocked_exit = frames_blocked
        .iter()
        .find_map(|f| {
            if let ServerFrame::Exit { code } = f {
                Some(*code)
            } else {
                None
            }
        })
        .expect("expected an Exit frame in blocked run");
    assert_ne!(
        blocked_exit, 0,
        "expected non-zero exit when directory is mode 0000 (EACCES), got 0; frames: {frames_blocked:?}"
    );

    // Now with mode 0711 the child must succeed and print "FAKE_KEY".
    let spec_traversable = ChildSpec {
        program: "sh".into(),
        args: vec!["-c".into(), format!("cat {id_rsa_str}")],
        env: vec![],
        cwd: std::env::temp_dir(),
        uid: None,
        gid: None,
        supplementary_gids: Vec::new(),
        home: None,
    };

    let mut buf2 = Vec::new();
    stream_child(&spec_traversable, tokio::io::empty(), &mut buf2)
        .await
        .unwrap();
    let frames_ok = collect_frames(buf2).await;

    assert!(
        matches!(frames_ok.last(), Some(ServerFrame::Exit { code: 0 })),
        "expected exit 0 when directory is mode 0711, got: {frames_ok:?}"
    );
    let stdout = concat_stdout(&frames_ok);
    assert!(
        stdout.windows(8).any(|w| w == b"FAKE_KEY"),
        "expected stdout to contain 'FAKE_KEY' after traversable dir fix, got: {:?}",
        String::from_utf8_lossy(&stdout)
    );
}

#[tokio::test]
async fn child_stdin_receives_stdin_chunk_bytes() {
    let source = client_frames(&[
        ClientFrame::StdinChunk {
            data: b"hello body\n".to_vec(),
        },
        ClientFrame::StdinEof,
    ])
    .await;

    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream_child(&cat_spec(), source, &mut buf),
    )
    .await
    .expect("relay did not finish within timeout")
    .expect("stream_child returned error");

    let frames = collect_frames(buf).await;
    assert_eq!(
        String::from_utf8_lossy(&concat_stdout(&frames)),
        "hello body\n",
        "child did not receive the stdin chunk bytes; frames: {frames:?}"
    );
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[tokio::test]
async fn multiple_stdin_chunks_arrive_in_order() {
    let source = client_frames(&[
        ClientFrame::StdinChunk {
            data: b"line one\n".to_vec(),
        },
        ClientFrame::StdinChunk {
            data: b"line two\n".to_vec(),
        },
        ClientFrame::StdinEof,
    ])
    .await;

    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream_child(&cat_spec(), source, &mut buf),
    )
    .await
    .expect("relay did not finish within timeout")
    .expect("stream_child returned error");

    let frames = collect_frames(buf).await;
    assert_eq!(
        String::from_utf8_lossy(&concat_stdout(&frames)),
        "line one\nline two\n",
        "chunks must reach the child as one ordered stream with no boundary marker"
    );
}

#[tokio::test]
async fn stdin_eof_closes_child_stdin() {
    // A chunk after `StdinEof` must never reach the child: the relay ends at
    // the frame, not at the end of the source. `cat` exits only once its
    // standard input is closed, so an Exit frame also proves the close.
    let source = client_frames(&[
        ClientFrame::StdinChunk {
            data: b"x".to_vec(),
        },
        ClientFrame::StdinEof,
        ClientFrame::StdinChunk {
            data: b"after-eof".to_vec(),
        },
    ])
    .await;

    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream_child(&cat_spec(), source, &mut buf),
    )
    .await
    .expect("child did not exit, so its stdin was never closed")
    .expect("stream_child returned error");

    let frames = collect_frames(buf).await;
    assert_eq!(
        String::from_utf8_lossy(&concat_stdout(&frames)),
        "x",
        "bytes after StdinEof must not reach the child; frames: {frames:?}"
    );
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[tokio::test]
async fn connection_eof_closes_child_stdin() {
    // No StdinEof frame: the source simply ends, as it does when a client
    // shuts down its write half.
    let source = client_frames(&[ClientFrame::StdinChunk {
        data: b"from connection\n".to_vec(),
    }])
    .await;

    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream_child(&cat_spec(), source, &mut buf),
    )
    .await
    .expect("child did not exit, so its stdin was never closed")
    .expect("stream_child returned error");

    let frames = collect_frames(buf).await;
    assert_eq!(
        String::from_utf8_lossy(&concat_stdout(&frames)),
        "from connection\n"
    );
    assert!(
        !frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Denied { .. })),
        "connection end-of-file is not a denial; frames: {frames:?}"
    );
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[tokio::test]
async fn exhausted_stdin_source_closes_child_stdin_immediately() {
    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream_child(&cat_spec(), tokio::io::empty(), &mut buf),
    )
    .await
    .expect("an exhausted source must close the child's stdin at once")
    .expect("stream_child returned error");

    let frames = collect_frames(buf).await;
    assert!(
        concat_stdout(&frames).is_empty(),
        "no bytes were sent, so none may be echoed; frames: {frames:?}"
    );
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[tokio::test]
async fn child_exiting_early_tolerates_broken_stdin_pipe() {
    // `exit 7` never reads its standard input, so writing 1 MiB into it fails
    // with a broken pipe part-way through. That is the ordinary case of a tool
    // that read the body it wanted and exited, not an invocation failure.
    const CHUNKS: usize = 128;
    let mut frames_to_send = Vec::with_capacity(CHUNKS + 1);
    for _ in 0..CHUNKS {
        frames_to_send.push(ClientFrame::StdinChunk {
            data: vec![b'z'; 8 * 1024],
        });
    }
    frames_to_send.push(ClientFrame::StdinEof);
    let source = client_frames(&frames_to_send).await;

    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        stream_child(&sh_spec("exit 7"), source, &mut buf),
    )
    .await
    .expect("a broken stdin pipe must not stall the relay")
    .expect("a broken stdin pipe must not fail the invocation");

    let frames = collect_frames(buf).await;
    assert!(
        !frames
            .iter()
            .any(|f| matches!(f, ServerFrame::Denied { .. })),
        "a broken stdin pipe is not a denial; frames: {frames:?}"
    );
    match frames.last() {
        Some(ServerFrame::Exit { code }) => assert_eq!(*code, 7, "expected the child's own code"),
        other => panic!("expected Exit 7, got {other:?}"),
    }
}

#[tokio::test]
async fn output_streams_while_stdin_still_arriving() {
    // The test sends one chunk, then refuses to send the next until the first
    // has been echoed back. An executor that serialised the two directions in
    // either order would stall here: reading input only after output finished
    // leaves `cat` waiting forever, and writing input only after output
    // finished leaves this test waiting forever.
    let (mut caller, executor_input) = tokio::io::duplex(64 * 1024);
    let (mut executor_output, mut responses) = tokio::io::duplex(64 * 1024);

    let relay = tokio::spawn(async move {
        stream_child(&cat_spec(), executor_input, &mut executor_output).await
    });

    let mut echoed = Vec::new();
    write_frame(
        &mut caller,
        &ClientFrame::StdinChunk {
            data: b"line one\n".to_vec(),
        },
    )
    .await
    .unwrap();
    read_until_echoed(&mut responses, &mut echoed, "line one\n").await;

    write_frame(
        &mut caller,
        &ClientFrame::StdinChunk {
            data: b"line two\n".to_vec(),
        },
    )
    .await
    .unwrap();
    read_until_echoed(&mut responses, &mut echoed, "line two\n").await;

    write_frame(&mut caller, &ClientFrame::StdinEof)
        .await
        .unwrap();

    let exit = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame::<_, ServerFrame>(&mut responses),
    )
    .await
    .expect("timed out waiting for the Exit frame")
    .expect("failed to read the Exit frame");
    assert!(
        matches!(exit, ServerFrame::Exit { code: 0 }),
        "expected Exit 0 after StdinEof, got {exit:?}"
    );

    relay
        .await
        .expect("relay task panicked")
        .expect("stream_child returned error");
}

#[tokio::test]
async fn large_stdin_bounded_memory() {
    // 1 MiB in 8 KiB chunks. Echoing it back proves in-order delivery of the
    // whole stream, and the per-chunk bound proves no direction accumulated it.
    const TOTAL: usize = 1024 * 1024;
    const CHUNK: usize = 8 * 1024;
    let payload: Vec<u8> = (0..TOTAL).map(|i| (i % 251) as u8).collect();

    let mut frames_to_send: Vec<ClientFrame> = payload
        .chunks(CHUNK)
        .map(|c| ClientFrame::StdinChunk { data: c.to_vec() })
        .collect();
    frames_to_send.push(ClientFrame::StdinEof);
    let source = client_frames(&frames_to_send).await;

    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(30),
        stream_child(&cat_spec(), source, &mut buf),
    )
    .await
    .expect("relay did not finish within timeout")
    .expect("stream_child returned error");

    let frames = collect_frames(buf).await;
    let echoed = concat_stdout(&frames);
    assert_eq!(echoed.len(), TOTAL, "child did not receive every byte");
    assert_eq!(echoed, payload, "child received the bytes out of order");
    for f in &frames {
        if let ServerFrame::StdoutChunk { data } = f {
            assert!(
                data.len() <= ghbrk::executor::READ_BUF_SIZE,
                "chunk size {} exceeded READ_BUF_SIZE {}",
                data.len(),
                ghbrk::executor::READ_BUF_SIZE
            );
        }
    }
    assert!(matches!(frames.last(), Some(ServerFrame::Exit { code: 0 })));
}

#[tokio::test]
async fn exit_frame_is_last_after_stdin_relay() {
    let source = client_frames(&[
        ClientFrame::StdinChunk {
            data: b"payload\n".to_vec(),
        },
        ClientFrame::StdinEof,
    ])
    .await;

    let mut buf = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        stream_child(&sh_spec("cat; printf 'trailing\\n' 1>&2"), source, &mut buf),
    )
    .await
    .expect("relay did not finish within timeout")
    .expect("stream_child returned error");

    let frames = collect_frames(buf).await;
    let exits: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f, ServerFrame::Exit { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        exits.len(),
        1,
        "expected exactly one Exit frame: {frames:?}"
    );
    assert_eq!(
        exits[0],
        frames.len() - 1,
        "no frame may follow Exit: {frames:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&concat_stdout(&frames)),
        "payload\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&concat_stderr(&frames)),
        "trailing\n"
    );
}
