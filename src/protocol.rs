use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum permitted frame body size: 16 MiB.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Tool the shim is brokering on behalf of the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Git,
    Gh,
    Check,
    Explain,
    Policy,
    Allow,
}

/// Request frame sent by the shim to the broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub tool: Tool,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Raw remote URL hint resolved by the shim (e.g. `git@github.com:org/repo.git`).
    #[serde(default)]
    pub remote_url: Option<String>,
    /// Head branch name hint resolved by the shim (e.g. `main`).
    #[serde(default)]
    pub head_branch: Option<String>,
    /// Whether one or more `ClientFrame`s follow this request on the same
    /// connection, carrying the caller's own standard input to the spawned
    /// child. Defaults to `false`, which is how every request from a client
    /// released before stdin forwarding existed decodes: the broker then
    /// closes the child's standard input at spawn time instead of waiting
    /// for a frame that will never arrive.
    #[serde(default)]
    pub client_frames: bool,
    /// Device and inode of the caller's own `/tmp`, sent only by the `check`
    /// request, for the `cli/doctor` shared-filesystem check. Absent by
    /// default. Carrying only two integers keeps the broker from opening any
    /// path on the caller's word — it compares them against a value it
    /// derives itself.
    #[serde(default)]
    pub caller_tmp: Option<TmpIdentity>,
}

/// Device and inode identifying one mounted filesystem, used to compare the
/// caller's `/tmp` against the broker's own view of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmpIdentity {
    pub dev: u64,
    pub ino: u64,
}

/// Frames the gateway client sends to the broker after the initial `Request`,
/// carrying the caller's own standard input to the spawned child. Kept out of
/// `Request` itself, and distinguished from it by position rather than by a
/// shared tag, so a client built before stdin forwarding existed still speaks
/// the current protocol for its first frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientFrame {
    StdinChunk { data: Vec<u8> },
    StdinEof,
}

/// Observed owner and mode of a single credential path, stat'd by the broker on
/// behalf of a caller who cannot stat it directly (paths under the broker's
/// credentials root are unreadable by the calling user).
///
/// New fields carry `#[serde(default)]` so a client built before they existed
/// can still deserialize the frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathAudit {
    /// Human-readable label for the path (e.g. `Credential dir`, `SSH key`).
    pub label: String,
    /// Absolute path the broker stat'd.
    pub path: PathBuf,
    /// Whether the path exists.
    pub present: bool,
    /// Owner uid observed by the broker. Meaningless when `present` is false.
    #[serde(default)]
    pub observed_owner_uid: u32,
    /// Permission bits (`st_mode & 0o777`) observed by the broker. Meaningless
    /// when `present` is false.
    #[serde(default)]
    pub observed_mode: u32,
}

/// Owner/mode audit of the caller's credential directory and credential files,
/// produced by the broker so the `doctor` client can run the tiered permission
/// classifier against paths it cannot stat itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CredentialAudit {
    #[serde(default)]
    pub entries: Vec<PathAudit>,
}

/// Frames the broker emits back to the shim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerFrame {
    StdoutChunk { data: Vec<u8> },
    StderrChunk { data: Vec<u8> },
    Exit { code: i32 },
    Denied { reason: String },
    CredentialAudit { audit: CredentialAudit },
}

/// Errors produced when reading/writing protocol frames.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("frame body length {0} exceeds {MAX_FRAME_LEN} byte ceiling")]
    FrameTooLarge(u32),
    #[error("frame body truncated: expected {expected} bytes, got {actual}")]
    Truncated { expected: u32, actual: u32 },
    #[error("invalid JSON in frame body: {0}")]
    InvalidJson(#[source] serde_json::Error),
    #[error("frame body too large to encode: {0} bytes")]
    EncodeTooLarge(usize),
}

/// Encode `value` as a length-prefixed JSON frame and write it to `writer`.
///
/// Wire format: 4-byte big-endian u32 length, then UTF-8 JSON body of that length.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(value).map_err(ProtocolError::InvalidJson)?;
    if body.len() > MAX_FRAME_LEN as usize {
        return Err(ProtocolError::EncodeTooLarge(body.len()));
    }
    let len = body.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&body).await?;
    Ok(())
}

/// Read one length-prefixed JSON frame from `reader` and deserialize it as `T`.
///
/// Rejects frames whose declared length exceeds [`MAX_FRAME_LEN`] without
/// attempting to allocate or read the body. Returns `Truncated` if EOF arrives
/// before the body has been fully read.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, ProtocolError>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);

    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(len));
    }

    let mut body = vec![0u8; len as usize];
    let mut filled = 0usize;
    while filled < body.len() {
        match reader.read(&mut body[filled..]).await {
            Ok(0) => {
                return Err(ProtocolError::Truncated {
                    expected: len,
                    actual: filled as u32,
                });
            }
            Ok(n) => filled += n,
            Err(e) => return Err(ProtocolError::Io(e)),
        }
    }

    serde_json::from_slice(&body).map_err(ProtocolError::InvalidJson)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    async fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let mut buf = Vec::new();
        write_frame(&mut buf, value).await.expect("encode");
        let mut cursor = Cursor::new(buf);
        read_frame(&mut cursor).await.expect("decode")
    }

    #[tokio::test]
    async fn check_request_round_trips_with_check_discriminant() {
        let req = Request {
            tool: Tool::Check,
            args: vec![],
            cwd: PathBuf::from("/"),
            remote_url: None,
            head_branch: None,
            client_frames: false,
            caller_tmp: None,
        };
        let decoded: Request = round_trip(&req).await;
        assert_eq!(decoded, req);
        let json = serde_json::to_string(&Tool::Check).unwrap();
        assert_eq!(json, r#""check""#);
    }

    #[tokio::test]
    async fn explain_request_round_trips() {
        let req = Request {
            tool: Tool::Explain,
            args: vec!["git".into(), "push".into(), "origin".into(), "main".into()],
            cwd: PathBuf::from("/work/repo"),
            remote_url: None,
            head_branch: None,
            client_frames: false,
            caller_tmp: None,
        };
        let decoded: Request = round_trip(&req).await;
        assert_eq!(decoded, req);
        assert_eq!(decoded.tool, Tool::Explain);
        let json = serde_json::to_string(&Tool::Explain).unwrap();
        assert_eq!(json, r#""explain""#);
    }

    #[tokio::test]
    async fn policy_request_round_trips() {
        let req = Request {
            tool: Tool::Policy,
            args: vec!["acme/web".into()],
            cwd: PathBuf::from("/work/repo"),
            remote_url: None,
            head_branch: None,
            client_frames: false,
            caller_tmp: None,
        };
        let decoded: Request = round_trip(&req).await;
        assert_eq!(decoded, req);
        assert_eq!(decoded.tool, Tool::Policy);
        let json = serde_json::to_string(&Tool::Policy).unwrap();
        assert_eq!(json, r#""policy""#);
    }

    #[tokio::test]
    async fn request_round_trip() {
        let original = Request {
            tool: Tool::Git,
            args: vec!["push".into(), "origin".into(), "main".into()],
            cwd: PathBuf::from("/work/repo"),
            remote_url: None,
            head_branch: None,
            client_frames: false,
            caller_tmp: None,
        };
        let decoded: Request = round_trip(&original).await;
        assert_eq!(decoded, original);
    }

    #[tokio::test]
    async fn request_round_trip_with_hints() {
        let original = Request {
            tool: Tool::Git,
            args: vec!["push".into(), "origin".into(), "main".into()],
            cwd: PathBuf::from("/work/repo"),
            remote_url: Some("git@github.com:acme/web.git".into()),
            head_branch: Some("main".into()),
            client_frames: false,
            caller_tmp: None,
        };
        let decoded: Request = round_trip(&original).await;
        assert_eq!(decoded, original);
        assert_eq!(
            decoded.remote_url.as_deref(),
            Some("git@github.com:acme/web.git")
        );
        assert_eq!(decoded.head_branch.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn request_without_hint_fields_defaults_to_none() {
        let legacy_json = r#"{"tool":"git","args":["status"],"cwd":"/work/repo"}"#;
        let decoded: Request = serde_json::from_str(legacy_json).expect("legacy decode");
        assert_eq!(decoded.remote_url, None);
        assert_eq!(decoded.head_branch, None);
    }

    #[tokio::test]
    async fn request_without_client_frames_field_decodes_as_false() {
        let legacy_json = r#"{"tool":"git","args":["status"],"cwd":"/work/repo"}"#;
        let decoded: Request = serde_json::from_str(legacy_json).expect("legacy decode");
        assert!(!decoded.client_frames);
    }

    #[tokio::test]
    async fn check_request_round_trips_with_caller_tmp() {
        let req = Request {
            tool: Tool::Check,
            args: vec![],
            cwd: PathBuf::from("/home/alice"),
            remote_url: None,
            head_branch: None,
            client_frames: false,
            caller_tmp: Some(TmpIdentity { dev: 64512, ino: 2 }),
        };
        let decoded: Request = round_trip(&req).await;
        assert_eq!(decoded, req);
        assert_eq!(decoded.caller_tmp, Some(TmpIdentity { dev: 64512, ino: 2 }));
    }

    #[tokio::test]
    async fn check_request_round_trips_without_caller_tmp() {
        let req = Request {
            tool: Tool::Check,
            args: vec![],
            cwd: PathBuf::from("/home/alice"),
            remote_url: None,
            head_branch: None,
            client_frames: false,
            caller_tmp: None,
        };
        let decoded: Request = round_trip(&req).await;
        assert_eq!(decoded, req);
        assert_eq!(decoded.caller_tmp, None);
    }

    #[tokio::test]
    async fn request_ignores_unknown_fields() {
        let json = r#"{"tool":"gh","args":["pr","create"],"cwd":"/work/repo","client_frames":true,"caller_tmp":null,"field_from_a_later_release":7}"#;
        let decoded: Request = serde_json::from_str(json).expect("decode with unknown field");
        assert_eq!(decoded.tool, Tool::Gh);
        assert_eq!(decoded.args, vec!["pr".to_string(), "create".to_string()]);
        assert!(decoded.client_frames);
        assert_eq!(decoded.caller_tmp, None);
    }

    #[tokio::test]
    async fn stdin_chunk_frame_round_trips() {
        let original = ClientFrame::StdinChunk {
            data: b"hello world!".to_vec(),
        };
        let decoded: ClientFrame = round_trip(&original).await;
        assert_eq!(decoded, original);
        if let ClientFrame::StdinChunk { data } = decoded {
            assert_eq!(data.len(), 12);
        } else {
            panic!("wrong variant");
        }
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains(r#""kind":"stdin_chunk""#));
    }

    #[tokio::test]
    async fn stdin_eof_frame_round_trips() {
        let original = ClientFrame::StdinEof;
        let decoded: ClientFrame = round_trip(&original).await;
        assert_eq!(decoded, original);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#"{"kind":"stdin_eof"}"#);
    }

    #[tokio::test]
    async fn bare_request_json_decodes_as_first_frame_without_kind() {
        let json = r#"{"tool":"gh","args":["pr","create"],"cwd":"/work/repo"}"#;
        let decoded: Request = serde_json::from_str(json).expect("bare request decode");
        assert_eq!(decoded.tool, Tool::Gh);
        assert!(!decoded.client_frames);
    }

    #[tokio::test]
    async fn stdout_chunk_decodes_bytes() {
        let original = ServerFrame::StdoutChunk {
            data: b"hello world!".to_vec(),
        };
        let decoded: ServerFrame = round_trip(&original).await;
        assert_eq!(decoded, original);
        if let ServerFrame::StdoutChunk { data } = decoded {
            assert_eq!(data.len(), 12);
        } else {
            panic!("wrong variant");
        }
    }

    #[tokio::test]
    async fn stderr_chunk_distinct_from_stdout() {
        let stderr = ServerFrame::StderrChunk {
            data: b"err".to_vec(),
        };
        let stdout = ServerFrame::StdoutChunk {
            data: b"err".to_vec(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &stderr).await.unwrap();
        let mut cursor = Cursor::new(buf);
        let decoded: ServerFrame = read_frame(&mut cursor).await.unwrap();
        assert_eq!(decoded, stderr);
        assert_ne!(decoded, stdout);
        assert!(matches!(decoded, ServerFrame::StderrChunk { .. }));
    }

    #[tokio::test]
    async fn exit_frame_terminates_stream() {
        let exit = ServerFrame::Exit { code: 7 };
        let decoded: ServerFrame = round_trip(&exit).await;
        assert_eq!(decoded, exit);
    }

    #[tokio::test]
    async fn denied_frame_carries_reason() {
        let denied = ServerFrame::Denied {
            reason: "branch main is protected".into(),
        };
        let decoded: ServerFrame = round_trip(&denied).await;
        match decoded {
            ServerFrame::Denied { reason } => {
                assert_eq!(reason, "branch main is protected");
            }
            _ => panic!("expected Denied"),
        }
    }

    #[tokio::test]
    async fn oversize_length_rejected() {
        let mut buf = Vec::new();
        let oversize: u32 = MAX_FRAME_LEN + 1;
        buf.extend_from_slice(&oversize.to_be_bytes());
        let mut cursor = Cursor::new(buf);
        let result: Result<ServerFrame, _> = read_frame(&mut cursor).await;
        match result {
            Err(ProtocolError::FrameTooLarge(len)) => assert_eq!(len, oversize),
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn truncated_body_parse_error() {
        let mut buf = Vec::new();
        let declared: u32 = 100;
        buf.extend_from_slice(&declared.to_be_bytes());
        buf.extend_from_slice(&[0u8; 40]);
        let mut cursor = Cursor::new(buf);
        let result: Result<ServerFrame, _> = read_frame(&mut cursor).await;
        match result {
            Err(ProtocolError::Truncated { expected, actual }) => {
                assert_eq!(expected, 100);
                assert_eq!(actual, 40);
            }
            other => panic!("expected Truncated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_json_returns_parse_error() {
        let mut buf = Vec::new();
        let body = b"not json";
        let len = body.len() as u32;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(body);
        let mut cursor = Cursor::new(buf);
        let result: Result<ServerFrame, _> = read_frame(&mut cursor).await;
        assert!(matches!(result, Err(ProtocolError::InvalidJson(_))));
    }

    #[tokio::test]
    async fn allow_request_round_trips_with_allow_discriminant() {
        let req = Request {
            tool: Tool::Allow,
            args: vec!["acme/web".into(), "write".into()],
            cwd: PathBuf::from("/work/repo"),
            remote_url: None,
            head_branch: None,
            client_frames: false,
            caller_tmp: None,
        };
        let decoded: Request = round_trip(&req).await;
        assert_eq!(decoded, req);
        assert_eq!(decoded.tool, Tool::Allow);
        let json = serde_json::to_string(&Tool::Allow).unwrap();
        assert_eq!(json, r#""allow""#);
    }

    #[tokio::test]
    async fn credential_audit_frame_round_trips() {
        let frame = ServerFrame::CredentialAudit {
            audit: CredentialAudit {
                entries: vec![
                    PathAudit {
                        label: "Credential dir".into(),
                        path: PathBuf::from("/var/lib/ghbrk/credentials/alice"),
                        present: true,
                        observed_owner_uid: 4242,
                        observed_mode: 0o700,
                    },
                    PathAudit {
                        label: "Token".into(),
                        path: PathBuf::from("/var/lib/ghbrk/credentials/alice/token"),
                        present: false,
                        observed_owner_uid: 0,
                        observed_mode: 0,
                    },
                ],
            },
        };
        let decoded: ServerFrame = round_trip(&frame).await;
        assert_eq!(decoded, frame);
    }

    #[tokio::test]
    async fn path_audit_without_owner_mode_fields_defaults_to_zero() {
        let legacy_json = r#"{"label":"Token","path":"/x","present":false}"#;
        let decoded: PathAudit = serde_json::from_str(legacy_json).expect("legacy decode");
        assert_eq!(decoded.observed_owner_uid, 0);
        assert_eq!(decoded.observed_mode, 0);
        assert!(!decoded.present);
    }

    #[tokio::test]
    async fn credential_audit_without_entries_defaults_to_empty() {
        let decoded: CredentialAudit = serde_json::from_str("{}").expect("legacy decode");
        assert!(decoded.entries.is_empty());
    }

    #[tokio::test]
    async fn empty_eof_returns_io_error() {
        let buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(buf);
        let result: Result<ServerFrame, _> = read_frame(&mut cursor).await;
        assert!(matches!(result, Err(ProtocolError::Io(_))));
    }

    #[tokio::test]
    async fn frame_at_max_length_succeeds() {
        let envelope = serde_json::to_vec(&ServerFrame::Denied {
            reason: String::new(),
        })
        .unwrap();
        let payload_len = MAX_FRAME_LEN as usize - envelope.len();
        let payload_string = "x".repeat(payload_len);
        let frame = ServerFrame::Denied {
            reason: payload_string.clone(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &frame).await.expect("encode");
        assert_eq!(buf.len(), 4 + MAX_FRAME_LEN as usize);
        let mut cursor = Cursor::new(buf);
        let decoded: ServerFrame = read_frame(&mut cursor).await.expect("decode");
        match decoded {
            ServerFrame::Denied { reason } => assert_eq!(reason, payload_string),
            _ => panic!("expected Denied"),
        }
    }
}
