//! Integration tests for the wire-protocol framing layer.
//!
//! Uses real Unix socket pairs to verify framing behaviour that `Cursor`-based
//! unit tests in `protocol.rs` cannot exercise: partial body delivery and
//! oversized-frame rejection over real async I/O.

use ghbrk::protocol::{read_frame, write_frame, ProtocolError, ServerFrame, MAX_FRAME_LEN};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// Verify that `read_frame` reassembles a body that arrives in two fragments.
///
/// The writer task sends the length header, yields, then writes each half of the
/// body with a yield in between. This forces the decoder's partial-read while
/// loop to handle real async I/O rather than a `Cursor` that delivers everything
/// at once.
#[tokio::test]
async fn partial_body_arrives_in_two_chunks_decodes_correctly() {
    let frame = ServerFrame::StdoutChunk {
        data: b"hello, framing!".to_vec(),
    };
    let body = serde_json::to_vec(&frame).unwrap();
    let len = body.len() as u32;
    let mid = body.len() / 2;

    let (mut writer, mut reader) = UnixStream::pair().unwrap();

    let body_clone = body.clone();
    let write_task = tokio::spawn(async move {
        writer.write_all(&len.to_be_bytes()).await.unwrap();
        tokio::task::yield_now().await;
        writer.write_all(&body_clone[..mid]).await.unwrap();
        tokio::task::yield_now().await;
        writer.write_all(&body_clone[mid..]).await.unwrap();
    });

    let decoded: ServerFrame = read_frame(&mut reader).await.expect("decode failed");
    write_task.await.unwrap();

    assert_eq!(decoded, frame);
}

/// Verify that `read_frame` correctly reassembles a body that arrives one byte
/// at a time — the most extreme form of partial delivery.
#[tokio::test]
async fn body_arriving_byte_by_byte_decodes_correctly() {
    let frame = ServerFrame::Exit { code: 42 };
    let body = serde_json::to_vec(&frame).unwrap();
    let len = body.len() as u32;

    let (mut writer, mut reader) = UnixStream::pair().unwrap();

    let write_task = tokio::spawn(async move {
        writer.write_all(&len.to_be_bytes()).await.unwrap();
        tokio::task::yield_now().await;
        for byte in &body {
            writer.write_all(&[*byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
    });

    let decoded: ServerFrame = read_frame(&mut reader).await.expect("decode failed");
    write_task.await.unwrap();

    assert_eq!(decoded, frame);
}

/// Verify that `read_frame` returns `ProtocolError::FrameTooLarge` when the
/// declared length exceeds `MAX_FRAME_LEN`, and does so without attempting to
/// read or allocate the body bytes.
///
/// The writer sends a few body bytes after the oversized header; if the decoder
/// attempted to read the full declared length it would stall waiting for data
/// that never arrives. The test completing quickly is itself evidence that the
/// body read was skipped.
#[tokio::test]
async fn oversized_declared_length_returns_frame_too_large() {
    let oversize: u32 = MAX_FRAME_LEN + 1;

    let (mut writer, mut reader) = UnixStream::pair().unwrap();

    let write_task = tokio::spawn(async move {
        writer.write_all(&oversize.to_be_bytes()).await.unwrap();
        // Write a handful of bytes — far fewer than the declared length.
        // If the decoder tries to read `oversize` bytes it will stall here.
        writer.write_all(&[0u8; 64]).await.unwrap();
    });

    let result: Result<ServerFrame, _> = read_frame(&mut reader).await;
    write_task.await.unwrap();

    match result {
        Err(ProtocolError::FrameTooLarge(n)) => assert_eq!(n, oversize),
        other => panic!("expected FrameTooLarge({oversize}), got {other:?}"),
    }
}

/// Verify that the 4-byte length prefix split across two socket reads is
/// handled correctly — confirming `read_exact` semantics over a real socket.
#[tokio::test]
async fn length_prefix_split_across_reads_decodes_correctly() {
    let frame = ServerFrame::Denied {
        reason: "policy denied".into(),
    };
    let body = serde_json::to_vec(&frame).unwrap();
    let len = body.len() as u32;
    let len_bytes = len.to_be_bytes();

    let (mut writer, mut reader) = UnixStream::pair().unwrap();

    let write_task = tokio::spawn(async move {
        // First two bytes of the 4-byte length prefix.
        writer.write_all(&len_bytes[..2]).await.unwrap();
        tokio::task::yield_now().await;
        // Remaining two bytes of the prefix plus the full body.
        writer.write_all(&len_bytes[2..]).await.unwrap();
        tokio::task::yield_now().await;
        writer.write_all(&body).await.unwrap();
    });

    let decoded: ServerFrame = read_frame(&mut reader).await.expect("decode failed");
    write_task.await.unwrap();

    assert_eq!(decoded, frame);
}

/// Verify that multiple well-formed frames sent sequentially over the same
/// socket are each decoded independently and in order.
#[tokio::test]
async fn sequential_frames_are_each_decoded_independently() {
    let frames = [
        ServerFrame::StdoutChunk {
            data: b"chunk one".to_vec(),
        },
        ServerFrame::StderrChunk {
            data: b"chunk two".to_vec(),
        },
        ServerFrame::Exit { code: 0 },
    ];

    let (mut writer, mut reader) = UnixStream::pair().unwrap();

    let frames_to_send = frames.clone();
    let write_task = tokio::spawn(async move {
        for frame in &frames_to_send {
            write_frame(&mut writer, frame).await.unwrap();
            tokio::task::yield_now().await;
        }
    });

    let first: ServerFrame = read_frame(&mut reader).await.unwrap();
    let second: ServerFrame = read_frame(&mut reader).await.unwrap();
    let third: ServerFrame = read_frame(&mut reader).await.unwrap();
    write_task.await.unwrap();

    assert_eq!(first, frames[0]);
    assert_eq!(second, frames[1]);
    assert_eq!(third, frames[2]);
}
