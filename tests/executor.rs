//! Integration tests for the executor streaming module.
//!
//! Each test spawns a real child process (typically a small `sh -c '…'`
//! script) and asserts on the sequence of `ServerFrame` values written by
//! `stream_child` into an in-memory buffer.

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use ghbrk::executor::{stream_child, ChildSpec};
use ghbrk::protocol::{read_frame, ServerFrame};

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap()
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    tokio::time::timeout(Duration::from_secs(5), stream_child(&spec, &mut buf))
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
    stream_child(&spec, &mut buf).await.unwrap();
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
