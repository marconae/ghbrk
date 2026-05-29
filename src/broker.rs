//! Broker server: accepts shim connections on a Unix socket, identifies the
//! caller via SO_PEERCRED, runs the resolver and policy engine, writes an
//! audit record, and either streams the executed child's output or sends a
//! `Denied` frame.
//!
//! Per-connection failures (malformed frames, unknown caller, resolver/policy
//! errors, executor failures) MUST NOT crash the daemon. The accept loop only
//! terminates on SIGINT, SIGTERM, or a fatal bind error.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
use nix::sys::stat::{umask, Mode};
use nix::unistd::{chown, Gid, Group, Uid, User};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{debug, error, info, warn};

use crate::audit::{AuditDecision, AuditLogger, AuditRecord};
use crate::credentials::{
    gh_env, https_git_env, load_credentials, ssh_env, CredentialError, Credentials,
};
use crate::executor::{stream_child, ChildSpec};
use crate::policy::{Decision, Operation, Policy, Request as PolicyRequest};
use crate::protocol::{read_frame, write_frame, ProtocolError, Request, ServerFrame, Tool};
use crate::resolver::{resolve_gh, resolve_git, ResolvedRequest, ResolverError, UrlScheme};

/// Socket mode: rw for owner and group, nothing for other.
pub const SOCKET_MODE: u32 = 0o660;

/// Group whose members are allowed to talk to the broker.
pub const CLIENT_GROUP_NAME: &str = "ghbrk-clients";

/// Configuration for [`run_broker`].
pub struct BrokerConfig {
    /// Filesystem path the broker should bind.
    pub socket_path: PathBuf,
    /// Loaded policy document.
    pub policy: Policy,
    /// Audit logger; shared with the daemon process for flush-on-shutdown.
    pub audit_logger: Arc<AuditLogger>,
    /// Optional credential root override (for tests). When `None`, the
    /// production default `/etc/ghbrk/credentials` is used.
    pub credentials_root: Option<PathBuf>,
}

/// Errors that bubble all the way out of [`run_broker`]. Per-connection errors
/// are handled internally and never become a `BrokerError`.
#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("failed to bind unix socket {path}: {source}")]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to set socket permissions: {0}")]
    Permissions(#[source] io::Error),
    #[error("failed to install signal handler: {0}")]
    Signal(#[source] io::Error),
}

/// Bind the broker socket and run the accept loop until SIGINT/SIGTERM.
///
/// On clean shutdown the audit log is flushed and the socket file is removed
/// before this function returns `Ok(())`.
pub async fn run_broker(config: BrokerConfig) -> Result<(), BrokerError> {
    let listener = bind_listener(&config.socket_path)?;
    apply_socket_permissions(&config.socket_path)?;
    apply_socket_group(&config.socket_path);

    info!(path = %config.socket_path.display(), "broker listening");

    let mut term = signal(SignalKind::terminate()).map_err(BrokerError::Signal)?;
    let mut int_sig = signal(SignalKind::interrupt()).map_err(BrokerError::Signal)?;

    let policy = Arc::new(config.policy);
    let credentials_root = config.credentials_root.clone().map(Arc::new);

    loop {
        tokio::select! {
            biased;
            _ = term.recv() => {
                info!("SIGTERM received, shutting down");
                break;
            }
            _ = int_sig.recv() => {
                info!("SIGINT received, shutting down");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let policy = Arc::clone(&policy);
                        let audit = Arc::clone(&config.audit_logger);
                        let cred_root = credentials_root.clone();
                        tokio::spawn(async move {
                            if let Err(err) = handle_connection(stream, policy, audit, cred_root).await {
                                debug!(error = %err, "connection terminated with error");
                            }
                        });
                    }
                    Err(err) => {
                        warn!(error = %err, "accept failed; continuing");
                    }
                }
            }
        }
    }

    if let Err(err) = config.audit_logger.flush() {
        warn!(error = %err, "audit flush on shutdown failed");
    }
    if let Err(err) = std::fs::remove_file(&config.socket_path) {
        if err.kind() != io::ErrorKind::NotFound {
            warn!(error = %err, "removing socket file failed");
        }
    }
    Ok(())
}

fn bind_listener(socket_path: &Path) -> Result<UnixListener, BrokerError> {
    // Set umask to 0o117 so the socket is created with at most 0o660 from the
    // start, closing the race window between bind and chmod.
    //
    // umask is process-wide, so we hold a lock to prevent concurrent calls
    // (e.g. in tests) from observing the wrong umask while we bind.
    let _guard = UMASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old_mask = umask(Mode::from_bits_truncate(0o117));
    let result = UnixListener::bind(socket_path).map_err(|source| BrokerError::Bind {
        path: socket_path.to_path_buf(),
        source,
    });
    umask(old_mask);
    result
}

static UMASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn apply_socket_permissions(socket_path: &Path) -> Result<(), BrokerError> {
    let perms = std::fs::Permissions::from_mode(SOCKET_MODE);
    std::fs::set_permissions(socket_path, perms).map_err(BrokerError::Permissions)
}

fn apply_socket_group(socket_path: &Path) {
    // Best-effort: only chgrp if the named group actually exists. Failing here
    // is recoverable for development setups.
    let group = match Group::from_name(CLIENT_GROUP_NAME) {
        Ok(Some(g)) => g,
        Ok(None) => {
            debug!(
                group = CLIENT_GROUP_NAME,
                "client group missing; leaving socket gid as default"
            );
            return;
        }
        Err(err) => {
            warn!(error = %err, "looking up {CLIENT_GROUP_NAME} failed");
            return;
        }
    };
    chown_socket_to_client_group(socket_path, group.gid);
}

/// Change the socket file's group to the resolved client-group GID. On
/// failure, emit an `error!`-level log naming the systemd `Group=` directive
/// that fixes the misconfiguration. Exposed for the broker integration tests.
pub fn chown_socket_to_client_group(socket_path: &Path, gid: Gid) {
    if let Err(err) = chown(socket_path, None, Some(gid)) {
        error!(
            error = %err,
            "chown socket to {CLIENT_GROUP_NAME} failed; \
             set Group=ghbrk-clients in the systemd unit to fix this"
        );
    }
}

/// Resolve the peer UID for a connected stream into a username. Returns `None`
/// if the kernel refuses the credentials lookup or the UID has no entry in
/// the password database.
pub fn peer_username(stream: &UnixStream) -> Option<String> {
    let cred = match getsockopt(&stream.as_fd(), PeerCredentials) {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "SO_PEERCRED failed");
            return None;
        }
    };
    let uid = Uid::from_raw(cred.uid());
    username_for_uid(uid)
}

/// Map a `Uid` to its Unix username via the password database. Returns `None`
/// when the UID has no matching user.
pub fn username_for_uid(uid: Uid) -> Option<String> {
    match User::from_uid(uid) {
        Ok(Some(user)) => Some(user.name),
        Ok(None) => None,
        Err(err) => {
            warn!(error = %err, "User::from_uid failed");
            None
        }
    }
}

async fn handle_connection(
    stream: UnixStream,
    policy: Arc<Policy>,
    audit: Arc<AuditLogger>,
    credentials_root: Option<Arc<PathBuf>>,
) -> Result<(), ConnectionError> {
    let username = match peer_username(&stream) {
        Some(name) => name,
        None => {
            let mut s = stream;
            send_denied(&mut s, "unknown caller").await.ok();
            return Ok(());
        }
    };
    debug!(user = %username, "connection accepted");
    let mut stream = stream;

    let request = match read_frame::<_, Request>(&mut stream).await {
        Ok(req) => req,
        Err(err) => {
            warn!(error = %err, user = %username, "malformed request frame");
            // Best-effort tell the client; ignore failures because the wire
            // may already be unrecoverable.
            send_denied(&mut stream, "malformed request").await.ok();
            return Ok(());
        }
    };

    process_request(
        &mut stream,
        request,
        &username,
        &policy,
        &audit,
        credentials_root.as_deref().map(|a| a.as_path()),
    )
    .await
}

async fn process_request(
    stream: &mut UnixStream,
    request: Request,
    username: &str,
    policy: &Policy,
    audit: &Arc<AuditLogger>,
    credentials_root: Option<&Path>,
) -> Result<(), ConnectionError> {
    // Short-circuit for Tool::Check — runs health checks as the broker user.
    // No resolver, no policy evaluation, no audit record.
    if request.tool == Tool::Check {
        return handle_check_request(stream, username, credentials_root).await;
    }

    // `gh` passthrough invocations (anything that is not a broker-op, e.g.
    // `gh repo view`, `gh auth status`) bypass resolve and policy but still
    // receive `GH_TOKEN` injection so the wrapped `gh` is authenticated.
    if request.tool == Tool::Gh && !crate::passthrough::gh_is_broker_op(&request.args) {
        return handle_gh_passthrough(stream, &request, username, audit, credentials_root).await;
    }

    let tool_name = match request.tool {
        Tool::Git => "git",
        Tool::Gh => "gh",
        Tool::Check => unreachable!("Tool::Check is handled before resolve_request"),
    };

    // Resolve the request to (org, repo, branch, operation).
    let resolved = match resolve_request(&request) {
        Ok(r) => r,
        Err(err) => {
            let reason = format!("resolver: {err}");
            write_audit(
                audit,
                AuditEntry {
                    user: username,
                    tool: tool_name,
                    args: &request.args,
                    org: "",
                    repo: "",
                    branch: None,
                    operation: "unresolved",
                    decision: AuditDecision::Deny {
                        reason: reason.clone(),
                    },
                },
            )
            .await;
            send_denied(stream, &reason).await?;
            return Ok(());
        }
    };

    let op_name = operation_name(&resolved.operation);
    let policy_req = PolicyRequest {
        user: username,
        org: &resolved.org,
        repo: &resolved.repo,
        operation: resolved.operation.clone(),
        branch: resolved.branch.as_deref(),
    };
    let decision = policy.evaluate(&policy_req);
    match &decision {
        Decision::Deny { reason } => {
            write_audit(
                audit,
                AuditEntry {
                    user: username,
                    tool: tool_name,
                    args: &request.args,
                    org: &resolved.org,
                    repo: &resolved.repo,
                    branch: resolved.branch.clone(),
                    operation: op_name,
                    decision: AuditDecision::Deny {
                        reason: reason.clone(),
                    },
                },
            )
            .await;
            send_denied(stream, reason).await?;
            return Ok(());
        }
        Decision::Allow => {
            write_audit(
                audit,
                AuditEntry {
                    user: username,
                    tool: tool_name,
                    args: &request.args,
                    org: &resolved.org,
                    repo: &resolved.repo,
                    branch: resolved.branch.clone(),
                    operation: op_name,
                    decision: AuditDecision::Allow,
                },
            )
            .await;
        }
    }

    // Build credential env vars.
    let creds = match load_user_credentials(username, credentials_root) {
        Ok(c) => c,
        Err(err) => {
            let reason = format!("credentials: {err}");
            send_denied(stream, &reason).await?;
            return Ok(());
        }
    };

    // The `_keepalive` binding holds the askpass tempfile alive for the
    // duration of the git invocation; dropping it removes the script.
    let (env, _keepalive) = match build_env(&request.tool, &resolved, &creds) {
        Ok(p) => p,
        Err(err) => {
            let reason = format!("credentials: {err}");
            send_denied(stream, &reason).await?;
            return Ok(());
        }
    };

    let spec = ChildSpec {
        program: tool_name.to_string(),
        args: request.args.clone(),
        env,
        cwd: request.cwd.clone(),
    };

    if let Err(err) = stream_child(&spec, stream).await {
        debug!(error = %err, "executor returned error");
    }
    let _ = stream.shutdown().await;
    Ok(())
}

async fn handle_gh_passthrough(
    stream: &mut UnixStream,
    request: &Request,
    username: &str,
    audit: &Arc<AuditLogger>,
    credentials_root: Option<&Path>,
) -> Result<(), ConnectionError> {
    let creds = match load_user_credentials(username, credentials_root) {
        Ok(c) => c,
        Err(err) => {
            let reason = format!("credentials: {err}");
            send_denied(stream, &reason).await?;
            return Ok(());
        }
    };

    write_audit(
        audit,
        AuditEntry {
            user: username,
            tool: "gh",
            args: &request.args,
            org: "",
            repo: "",
            branch: None,
            operation: "passthrough",
            decision: AuditDecision::Passthrough,
        },
    )
    .await;

    let spec = ChildSpec {
        program: "gh".to_string(),
        args: request.args.clone(),
        env: gh_env(&creds),
        cwd: request.cwd.clone(),
    };

    if let Err(err) = stream_child(&spec, stream).await {
        debug!(error = %err, "executor returned error");
    }
    let _ = stream.shutdown().await;
    Ok(())
}

async fn handle_check_request(
    stream: &mut UnixStream,
    username: &str,
    credentials_root: Option<&Path>,
) -> Result<(), ConnectionError> {
    let creds_root = match credentials_root {
        Some(r) => r.to_path_buf(),
        None => crate::credentials::credentials_dir(),
    };

    let mut output: Vec<u8> = Vec::new();
    let all_ok = crate::health_check::run_checks(&creds_root, username, &mut output);

    if !output.is_empty() {
        write_frame(stream, &ServerFrame::StdoutChunk { data: output }).await?;
    }

    let code = if all_ok { 0 } else { 1 };
    write_frame(stream, &ServerFrame::Exit { code }).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn resolve_request(request: &Request) -> Result<ResolvedRequest, ResolverError> {
    match request.tool {
        Tool::Git => resolve_git(&request.args, &request.cwd),
        Tool::Gh => resolve_gh(&request.args, &request.cwd),
        Tool::Check => unreachable!("Tool::Check is handled before resolve_request"),
    }
}

fn load_user_credentials(
    username: &str,
    root: Option<&Path>,
) -> Result<Credentials, CredentialError> {
    match root {
        Some(base) => crate::credentials::load_credentials_from(base, username),
        None => load_credentials(username),
    }
}

/// Pair of (env var bindings, optional RAII keep-alive guard).
type BuiltEnv = (
    Vec<(String, String)>,
    Option<crate::credentials::HttpsGitEnv>,
);

/// Build the env-var pairs for the chosen tool and URL scheme. Returns the
/// vars plus an optional keep-alive RAII guard (for the HTTPS askpass script).
fn build_env(
    tool: &Tool,
    resolved: &ResolvedRequest,
    creds: &Credentials,
) -> Result<BuiltEnv, CredentialError> {
    match tool {
        Tool::Gh => Ok((gh_env(creds), None)),
        Tool::Git => match resolved.url_scheme {
            UrlScheme::Ssh => Ok((ssh_env(creds), None)),
            UrlScheme::Https => {
                let env = https_git_env(creds)?;
                let vars = env.vars.clone();
                Ok((vars, Some(env)))
            }
        },
        Tool::Check => unreachable!("Tool::Check is handled before build_env"),
    }
}

fn operation_name(op: &Operation) -> &'static str {
    match op {
        Operation::Push => "push",
        Operation::Fetch => "fetch",
        Operation::Pull => "pull",
        Operation::Clone => "clone",
        Operation::PrOpen => "pr_open",
        Operation::PrComment => "pr_comment",
        Operation::PrClose => "pr_close",
        Operation::PrMerge => "pr_merge",
        Operation::PrReview => "pr_review",
        Operation::IssueOpen => "issue_open",
        Operation::IssueComment => "issue_comment",
        Operation::IssueClose => "issue_close",
        Operation::ReleaseCreate => "release_create",
        Operation::GhApiRead { .. } => "gh_api_read",
    }
}

struct AuditEntry<'a> {
    user: &'a str,
    tool: &'a str,
    args: &'a [String],
    org: &'a str,
    repo: &'a str,
    branch: Option<String>,
    operation: &'a str,
    decision: AuditDecision,
}

async fn write_audit(audit: &Arc<AuditLogger>, entry: AuditEntry<'_>) {
    let record = AuditRecord {
        timestamp: AuditRecord::now_timestamp(),
        user: entry.user.to_string(),
        tool: entry.tool.to_string(),
        args: entry.args.to_vec(),
        org: entry.org.to_string(),
        repo: entry.repo.to_string(),
        branch: entry.branch,
        operation: entry.operation.to_string(),
        decision: entry.decision,
    };
    let logger = Arc::clone(audit);
    let join = tokio::task::spawn_blocking(move || logger.write(&record)).await;
    match join {
        Ok(Ok(())) => {}
        Ok(Err(err)) => error!(error = %err, "audit write failed"),
        Err(err) => error!(error = %err, "audit write task join failed"),
    }
}

async fn send_denied<W>(writer: &mut W, reason: &str) -> Result<(), ProtocolError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    write_frame(
        writer,
        &ServerFrame::Denied {
            reason: reason.to_string(),
        },
    )
    .await
}

#[derive(Debug, Error)]
enum ConnectionError {
    #[error("protocol: {0}")]
    Protocol(#[from] ProtocolError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_uid_resolves_to_username() {
        let uid = Uid::current();
        let name = username_for_uid(uid).expect("current process must have a user");
        assert!(!name.is_empty());
    }

    #[test]
    fn unknown_uid_returns_none() {
        // Pick a UID that is unlikely to exist on any developer system.
        let candidate = Uid::from_raw(0x7FFF_FFFE);
        // If this happens to resolve (unlikely), skip the assertion.
        if let Some(name) = username_for_uid(candidate) {
            eprintln!("UID {candidate} unexpectedly resolved to {name}; skipping");
            return;
        }
        assert!(username_for_uid(candidate).is_none());
    }

    #[test]
    fn operation_name_covers_all_variants() {
        // Compile-time exhaustive: ensure no variant is missed by ensuring at
        // least the common ones produce non-empty strings.
        for op in [
            Operation::Push,
            Operation::Fetch,
            Operation::Pull,
            Operation::Clone,
            Operation::PrOpen,
            Operation::PrComment,
            Operation::PrMerge,
            Operation::PrClose,
            Operation::PrReview,
            Operation::IssueOpen,
            Operation::IssueComment,
            Operation::IssueClose,
            Operation::ReleaseCreate,
            Operation::GhApiRead {
                path: "user".to_string(),
            },
        ] {
            assert!(!operation_name(&op).is_empty(), "missing name for {op:?}");
        }
    }
}
