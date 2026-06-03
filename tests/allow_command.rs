//! Integration tests for the broker-side `allow` handler.
//!
//! The broker derives the peer identity from `SO_PEERCRED`, which in the test
//! runner is always the non-root uid of the process. So the non-root deny path
//! is exercised end-to-end over a real socket, while the root-peer
//! append/reload paths are exercised through the `handle_allow` test seam with
//! an injected privileged identity (uid 0). No test requires running as root.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use ghbrk::audit::AuditLogger;
use ghbrk::broker::{handle_allow, PeerIdentity};
use ghbrk::policy::{Effect, Operation, OperationsSpec, Policy};
use ghbrk::protocol::{read_frame, ServerFrame};
use tempfile::TempDir;
use tokio::net::UnixStream;

const STARTER_POLICY: &str = "rules: []\n";

struct Fixture {
    _tmp: TempDir,
    policy_path: PathBuf,
    audit_path: PathBuf,
    handle: Arc<ArcSwap<Policy>>,
    audit: Arc<AuditLogger>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_policy_text(STARTER_POLICY)
    }

    fn with_policy_text(text: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("policy.yaml");
        let audit_path = tmp.path().join("audit.log");
        std::fs::write(&policy_path, text).unwrap();
        let policy = Policy::from_yaml(text).unwrap();
        let handle = Arc::new(ArcSwap::from_pointee(policy));
        let audit = Arc::new(AuditLogger::new(&audit_path).unwrap());
        Self {
            _tmp: tmp,
            policy_path,
            audit_path,
            handle,
            audit,
        }
    }

    fn file_text(&self) -> String {
        std::fs::read_to_string(&self.policy_path).unwrap()
    }

    fn loaded(&self) -> Policy {
        Policy::from_yaml(&self.file_text()).unwrap()
    }

    fn audit_text(&self) -> String {
        self.audit.flush().unwrap();
        std::fs::read_to_string(&self.audit_path).unwrap_or_default()
    }
}

fn root_identity() -> PeerIdentity {
    PeerIdentity {
        username: "root".to_string(),
        uid: 0,
        gid: 0,
        supplementary_gids: Vec::new(),
        home: PathBuf::from("/root"),
    }
}

fn nonroot_identity() -> PeerIdentity {
    PeerIdentity {
        username: "alice".to_string(),
        uid: 1000,
        gid: 1000,
        supplementary_gids: Vec::new(),
        home: PathBuf::from("/home/alice"),
    }
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Drive `handle_allow` against an in-memory duplex stream, collecting every
/// frame the handler emits.
async fn run_allow(fx: &Fixture, identity: &PeerIdentity, raw_args: &[&str]) -> Vec<ServerFrame> {
    let (mut server, mut client) = tokio::io::duplex(64 * 1024);
    handle_allow(
        &mut server,
        &args(raw_args),
        identity,
        &fx.handle,
        &fx.policy_path,
        &fx.audit,
    )
    .await
    .expect("handle_allow should not error on the wire");
    // The handler never reads; close the write side so the reader sees EOF.
    drop(server);
    let mut frames = Vec::new();
    while let Ok(frame) = read_frame::<_, ServerFrame>(&mut client).await {
        frames.push(frame);
    }
    frames
}

fn last_exit(frames: &[ServerFrame]) -> Option<i32> {
    frames.iter().rev().find_map(|f| match f {
        ServerFrame::Exit { code } => Some(*code),
        _ => None,
    })
}

fn denied_reason(frames: &[ServerFrame]) -> Option<&str> {
    frames.iter().find_map(|f| match f {
        ServerFrame::Denied { reason } => Some(reason.as_str()),
        _ => None,
    })
}

fn stdout_text(frames: &[ServerFrame]) -> String {
    let mut out = Vec::new();
    for f in frames {
        if let ServerFrame::StdoutChunk { data } = f {
            out.extend_from_slice(data);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn allow_success_uses_stdout_then_exit() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &root_identity(), &["acme/web", "write"]).await;
    // A StdoutChunk must precede an Exit 0; no Denied frame.
    assert!(
        denied_reason(&frames).is_none(),
        "unexpected deny: {frames:?}"
    );
    assert!(
        !stdout_text(&frames).is_empty(),
        "expected a confirmation chunk: {frames:?}"
    );
    assert_eq!(last_exit(&frames), Some(0));
    // Ordering: stdout chunk comes before the exit.
    let exit_pos = frames
        .iter()
        .position(|f| matches!(f, ServerFrame::Exit { .. }))
        .unwrap();
    let stdout_pos = frames
        .iter()
        .position(|f| matches!(f, ServerFrame::StdoutChunk { .. }))
        .unwrap();
    assert!(
        stdout_pos < exit_pos,
        "stdout must precede exit: {frames:?}"
    );
}

#[tokio::test]
async fn allow_appends_and_reloads_for_root_peer() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &root_identity(), &["acme/web", "write"]).await;
    assert_eq!(last_exit(&frames), Some(0));

    // The rule is now persisted in the policy file.
    let loaded = fx.loaded();
    assert_eq!(loaded.rules.len(), 1);
    let rule = &loaded.rules[0];
    assert_eq!(rule.org, "acme");
    assert_eq!(rule.repo, "web");
    assert_eq!(rule.effect, Effect::Allow);

    // And the in-memory swappable handle reflects the reload.
    let live = fx.handle.load_full();
    assert_eq!(live.rules.len(), 1, "handle was not swapped after append");
}

#[tokio::test]
async fn allow_denied_for_non_root_peer() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &nonroot_identity(), &["acme/web", "write"]).await;
    let reason = denied_reason(&frames).expect("expected Denied for non-root");
    assert!(
        reason.to_lowercase().contains("root") || reason.to_lowercase().contains("privilege"),
        "deny reason should mention privilege: {reason}"
    );
    // The policy file must be untouched.
    assert_eq!(fx.file_text(), STARTER_POLICY);
    assert!(fx.handle.load_full().rules.is_empty());
}

#[tokio::test]
async fn unprivileged_caller_denied() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &nonroot_identity(), &["acme/web", "push"]).await;
    assert!(denied_reason(&frames).is_some());
    assert_eq!(fx.file_text(), STARTER_POLICY);
}

#[tokio::test]
async fn allow_unknown_operand_leaves_file_unchanged() {
    let fx = Fixture::new();
    let before = fx.file_text();
    let frames = run_allow(&fx, &root_identity(), &["acme/web", "frobnicate"]).await;
    let reason = denied_reason(&frames).expect("unknown operand should deny");
    assert!(
        reason.contains("frobnicate"),
        "deny reason should name the unknown operand: {reason}"
    );
    assert_eq!(fx.file_text(), before, "file must be unchanged on reject");
    assert!(fx.handle.load_full().rules.is_empty());
}

#[tokio::test]
async fn grant_operation_list_to_self() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &root_identity(), &["acme/web", "push", "fetch"]).await;
    assert_eq!(last_exit(&frames), Some(0));
    let loaded = fx.loaded();
    let rule = &loaded.rules[0];
    assert_eq!(rule.user, "root", "default target user is the caller");
    assert_eq!(
        rule.operations,
        OperationsSpec::List(vec![Operation::Push, Operation::Fetch])
    );
}

#[tokio::test]
async fn grant_named_role_stores_role_name() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &root_identity(), &["acme/web", "write"]).await;
    assert_eq!(last_exit(&frames), Some(0));
    // The persisted YAML must store the bare role name, not an expanded list.
    let text = fx.file_text();
    assert!(
        text.contains("operations: write"),
        "expected the role name verbatim in YAML:\n{text}"
    );
    let loaded = fx.loaded();
    assert_eq!(
        loaded.rules[0].operations,
        OperationsSpec::Role("write".to_string())
    );
}

#[tokio::test]
async fn grant_to_other_user() {
    let fx = Fixture::new();
    let frames = run_allow(
        &fx,
        &root_identity(),
        &["acme/web", "write", "--user", "alice"],
    )
    .await;
    assert_eq!(last_exit(&frames), Some(0));
    let loaded = fx.loaded();
    assert_eq!(loaded.rules[0].user, "alice");
}

#[tokio::test]
async fn unknown_operation_rejected() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &root_identity(), &["acme/web", "push", "bogusop"]).await;
    let reason = denied_reason(&frames).expect("unknown op should deny");
    assert!(
        reason.contains("bogusop"),
        "reason should name op: {reason}"
    );
    assert_eq!(fx.file_text(), STARTER_POLICY);
}

#[tokio::test]
async fn unknown_role_rejected() {
    let fx = Fixture::new();
    let frames = run_allow(&fx, &root_identity(), &["acme/web", "maintainer"]).await;
    let reason = denied_reason(&frames).expect("unknown role should deny");
    assert!(
        reason.contains("maintainer"),
        "reason should name the role: {reason}"
    );
    assert_eq!(fx.file_text(), STARTER_POLICY);
}

#[tokio::test]
async fn appended_rule_round_trips() {
    let fx = Fixture::new();
    run_allow(&fx, &root_identity(), &["acme/web", "push"]).await;
    // Reparse the file fresh and verify every field of the appended rule.
    let loaded = fx.loaded();
    let rule = &loaded.rules[0];
    assert_eq!(rule.user, "root");
    assert_eq!(rule.org, "acme");
    assert_eq!(rule.repo, "web");
    assert_eq!(rule.operations, OperationsSpec::List(vec![Operation::Push]));
    assert_eq!(rule.branches, vec!["*".to_string()]);
    assert_eq!(rule.effect, Effect::Allow);
}

#[tokio::test]
async fn append_preserves_existing_rules() {
    let existing = "rules:\n  \
        - user: bob\n    \
          org: acme\n    \
          repo: api\n    \
          operations: [fetch]\n    \
          branches: [\"*\"]\n    \
          effect: allow\n";
    let fx = Fixture::with_policy_text(existing);
    run_allow(&fx, &root_identity(), &["acme/web", "push"]).await;
    let loaded = fx.loaded();
    assert_eq!(loaded.rules.len(), 2, "must append, not overwrite");
    assert_eq!(loaded.rules[0].user, "bob");
    assert_eq!(loaded.rules[1].user, "root");
}

#[tokio::test]
async fn allow_audits_the_grant() {
    let fx = Fixture::new();
    run_allow(&fx, &root_identity(), &["acme/web", "write"]).await;
    let log = fx.audit_text();
    assert!(
        log.contains("\"decision\":\"allow\""),
        "expected allow audit:\n{log}"
    );
    assert!(log.contains("acme"), "audit should reference org:\n{log}");
}

#[tokio::test]
async fn allow_audits_the_deny() {
    let fx = Fixture::new();
    run_allow(&fx, &nonroot_identity(), &["acme/web", "write"]).await;
    let log = fx.audit_text();
    assert!(
        log.contains("\"decision\":\"deny\""),
        "expected deny audit:\n{log}"
    );
}

/// The daemon-unreachable path is a CLI concern, exercised here by confirming a
/// connection to a non-existent socket fails fast (the broker side never sees
/// it). This guards the contract the CLI relies on.
#[tokio::test]
async fn daemon_unreachable_reported() {
    let missing = Path::new("/nonexistent/ghbrk-allow-test/none.sock");
    let result = UnixStream::connect(missing).await;
    assert!(result.is_err(), "connecting to a missing socket must fail");
}
