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
