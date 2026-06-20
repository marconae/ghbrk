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

use arc_swap::ArcSwap;
use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
use nix::sys::stat::{umask, Mode};
use nix::unistd::{chown, getgrouplist, Gid, Group, Uid, User};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{debug, error, info, warn};

use crate::audit::{AuditDecision, AuditLogger, AuditRecord};
use crate::credentials::{
    gh_env, https_git_env, load_credentials, start_ssh_agent, CredentialError, Credentials,
    SshAgentHandle,
};
use crate::executor::{stream_child, ChildSpec};
use crate::policy::{
    Decision, Effect, Operation, OperationsSpec, Policy, Request as PolicyRequest, Rule,
};
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
    /// Swappable policy handle. Each connection reads a snapshot; the allow
    /// handler hot-reloads by storing a fresh `Arc<Policy>` into this handle.
    pub policy: Arc<ArcSwap<Policy>>,
    /// Path to the policy file on disk. The allow handler appends a rule here
    /// and reloads it into the swappable handle.
    pub policy_path: PathBuf,
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

    let policy_handle = Arc::clone(&config.policy);
    let policy_path = Arc::new(config.policy_path.clone());
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
                        // Snapshot the current policy at accept time. In-flight
                        // connections keep their snapshot across a later swap.
                        // The swappable handle and policy path travel alongside
                        // so the allow handler can hot-reload after a write.
                        let policy = policy_handle.load_full();
                        let handle = Arc::clone(&policy_handle);
                        let path = Arc::clone(&policy_path);
                        let audit = Arc::clone(&config.audit_logger);
                        let cred_root = credentials_root.clone();
                        tokio::spawn(async move {
                            let ctx = ConnectionContext {
                                policy,
                                policy_handle: handle,
                                policy_path: path,
                                audit,
                                credentials_root: cred_root,
                            };
                            if let Err(err) = handle_connection(stream, ctx).await {
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
    // Set umask to 0o077 so the socket is created with no group/other access
    // from the start, closing the race window between bind and chmod. The
    // owner-execute bit is deliberately left unmasked: umask is process-wide,
    // so during this brief window any file/directory another thread creates
    // (notably temp dirs in the test suite) would otherwise lose its
    // owner-traverse bit and become unusable. `apply_socket_permissions` widens
    // the socket back to the intended 0o660 immediately after bind.
    //
    // umask is process-wide, so we hold a lock to prevent concurrent calls
    // (e.g. in tests) from observing the wrong umask while we bind.
    let _guard = UMASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old_mask = umask(Mode::from_bits_truncate(0o077));
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

/// Resolved Unix identity of a connected peer, derived from `SO_PEERCRED` and
/// the password database. Carries everything the executor needs to drop
/// privileges to the calling user.
pub struct PeerIdentity {
    /// Login name from the password database.
    pub username: String,
    /// Numeric user ID.
    pub uid: u32,
    /// Primary numeric group ID.
    pub gid: u32,
    /// Supplementary group IDs (excludes the primary GID semantics handled by
    /// the kernel). Empty when enumeration failed.
    pub supplementary_gids: Vec<u32>,
    /// Home directory from the password database.
    pub home: PathBuf,
}

/// Resolve the peer of a connected stream into a full Unix identity. Returns
/// `None` (deny) when the kernel refuses the credentials lookup or the UID has
/// no entry in the password database. A failure to enumerate supplementary
/// groups is non-fatal: it degrades to an empty supplementary list while the
/// primary GID still applies.
pub fn peer_identity(stream: &UnixStream) -> Option<PeerIdentity> {
    let cred = match getsockopt(&stream.as_fd(), PeerCredentials) {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "SO_PEERCRED failed");
            return None;
        }
    };
    let uid = Uid::from_raw(cred.uid());
    let user = match User::from_uid(uid) {
        Ok(Some(user)) => user,
        Ok(None) => return None,
        Err(err) => {
            warn!(error = %err, "User::from_uid failed");
            return None;
        }
    };

    let supplementary_gids = supplementary_gids_for(&user.name, user.gid);

    Some(PeerIdentity {
        username: user.name,
        uid: user.uid.as_raw(),
        gid: user.gid.as_raw(),
        supplementary_gids,
        home: user.dir,
    })
}

/// Enumerate the supplementary groups for a user via `getgrouplist`. Returns an
/// empty list (and logs a warning) when the username cannot be turned into a
/// `CString` or the lookup fails; the primary GID is unaffected.
fn supplementary_gids_for(username: &str, primary: Gid) -> Vec<u32> {
    let name = match std::ffi::CString::new(username) {
        Ok(name) => name,
        Err(err) => {
            warn!(error = %err, user = %username, "username not representable as CString");
            return Vec::new();
        }
    };
    match getgrouplist(&name, primary) {
        Ok(groups) => groups.into_iter().map(|g| g.as_raw()).collect(),
        Err(err) => {
            warn!(error = %err, user = %username, "getgrouplist failed; proceeding with no supplementary groups");
            Vec::new()
        }
    }
}

/// Resolve the peer UID for a connected stream into a username. Returns `None`
/// if the kernel refuses the credentials lookup or the UID has no entry in
/// the password database.
pub fn peer_username(stream: &UnixStream) -> Option<String> {
    peer_identity(stream).map(|identity| identity.username)
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

/// Everything a single connection's handler needs from the daemon: the policy
/// snapshot taken at accept time, the swappable handle and on-disk path used by
/// the allow handler to hot-reload after a write, the audit logger, and the
/// optional credentials-root override.
struct ConnectionContext {
    policy: Arc<Policy>,
    policy_handle: Arc<ArcSwap<Policy>>,
    policy_path: Arc<PathBuf>,
    audit: Arc<AuditLogger>,
    credentials_root: Option<Arc<PathBuf>>,
}

async fn handle_connection(
    stream: UnixStream,
    ctx: ConnectionContext,
) -> Result<(), ConnectionError> {
    let identity = match peer_identity(&stream) {
        Some(identity) => identity,
        None => {
            let mut s = stream;
            send_denied(&mut s, "unknown caller").await.ok();
            return Ok(());
        }
    };
    debug!(user = %identity.username, "connection accepted");
    let mut stream = stream;

    let request = match read_frame::<_, Request>(&mut stream).await {
        Ok(req) => req,
        Err(err) => {
            warn!(error = %err, user = %identity.username, "malformed request frame");
            // Best-effort tell the client; ignore failures because the wire
            // may already be unrecoverable.
            send_denied(&mut stream, "malformed request").await.ok();
            return Ok(());
        }
    };

    process_request(&mut stream, request, &identity, &ctx).await
}

async fn process_request(
    stream: &mut UnixStream,
    request: Request,
    identity: &PeerIdentity,
    ctx: &ConnectionContext,
) -> Result<(), ConnectionError> {
    let policy = ctx.policy.as_ref();
    let audit = &ctx.audit;
    let credentials_root = ctx.credentials_root.as_deref().map(|a| a.as_path());
    let username = identity.username.as_str();

    // The privileged `allow` mutation routes before resolve/policy: it neither
    // resolves a repo URL nor consults the (read-path) policy. It validates the
    // grant against the live role vocabulary, appends to the policy file, and
    // hot-reloads the swappable handle.
    if request.tool == Tool::Allow {
        handle_allow(
            stream,
            &request.args,
            identity,
            &ctx.policy_handle,
            ctx.policy_path.as_path(),
            audit,
        )
        .await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    // Query tools resolve + evaluate policy without executing anything.
    if request.tool == Tool::Explain {
        return handle_explain_request(stream, &request, username, policy).await;
    }
    if request.tool == Tool::Policy {
        return handle_policy_request(stream, &request, username, policy).await;
    }

    // Short-circuit for Tool::Check — runs health checks as the broker user.
    // No resolver, no policy evaluation, no audit record.
    if request.tool == Tool::Check {
        return handle_check_request(stream, username, credentials_root).await;
    }

    // `gh` passthrough invocations (anything that is not a broker-op, e.g.
    // `gh repo view`, `gh auth status`) bypass resolve and policy but still
    // receive `GH_TOKEN` injection so the wrapped `gh` is authenticated.
    if request.tool == Tool::Gh && !gh_is_broker_op(&request.args) {
        return handle_gh_passthrough(stream, &request, identity, audit, credentials_root).await;
    }

    // Defence-in-depth: the `ghbrk git` gateway already filters local-only
    // subcommands client-side, but a hand-crafted client could still submit
    // one. Reject it here before any resolve or execution.
    if request.tool == Tool::Git && !git_is_remote_op(&request.args) {
        let reason = "local git operations must be run directly, not through ghbrk";
        write_audit(
            audit,
            AuditEntry {
                user: username,
                tool: "git",
                args: &request.args,
                org: "",
                repo: "",
                branch: None,
                operation: "local",
                decision: AuditDecision::Deny {
                    reason: reason.to_string(),
                },
            },
        )
        .await;
        send_denied(stream, reason).await?;
        return Ok(());
    }

    let tool_name = match request.tool {
        Tool::Git => "git",
        Tool::Gh => "gh",
        Tool::Check => unreachable!("Tool::Check is handled before resolve_request"),
        Tool::Explain | Tool::Policy => {
            unreachable!("query tools are handled before resolve_request")
        }
        Tool::Allow => unreachable!("Tool::Allow is handled before resolve_request"),
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
    // duration of the git invocation; dropping it removes the script. The
    // `_agent_keepalive` binding holds the SSH agent escrow alive for the
    // duration of `stream_child()`; dropping it kills the agent and removes
    // its temp dir (socket included).
    let (env, _keepalive, _agent_keepalive) =
        match build_env(&request.tool, &resolved, &creds).await {
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
        uid: Some(identity.uid),
        gid: Some(identity.gid),
        supplementary_gids: identity.supplementary_gids.clone(),
        home: Some(identity.home.clone()),
    };

    if let Err(err) = stream_child(&spec, stream).await {
        debug!(error = %err, "executor returned error");
    }
    let _ = stream.shutdown().await;
    Ok(())
}

/// Handle a privileged `allow` request: gate on root, parse and validate the
/// grant against the live role vocabulary, append the rule to the policy file
/// atomically, hot-reload the swappable handle, and stream a confirmation.
///
/// Generic over the writer so it can be driven by the broker's `UnixStream` in
/// production and by an in-memory duplex stream in tests. Never reads from the
/// stream and never spawns a subprocess. A returned `Ok(())` means every frame
/// was written cleanly, regardless of whether the grant was allowed or denied.
pub async fn handle_allow<W>(
    writer: &mut W,
    args: &[String],
    identity: &PeerIdentity,
    policy_handle: &Arc<ArcSwap<Policy>>,
    policy_path: &Path,
    audit: &Arc<AuditLogger>,
) -> Result<(), ProtocolError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let username = identity.username.as_str();

    // Privilege gate: only an effective UID 0 peer may mutate the policy. This
    // keeps the trust boundary on the privileged daemon, per the mission's
    // privilege-separation model.
    if identity.uid != 0 {
        let reason = "allow requires elevated privileges (root)".to_string();
        audit_allow(audit, username, args, "", "", &deny(&reason)).await;
        return send_denied(writer, &reason).await;
    }

    let request = match parse_allow_args(args, username) {
        Ok(req) => req,
        Err(reason) => {
            audit_allow(audit, username, args, "", "", &deny(&reason)).await;
            return send_denied(writer, &reason).await;
        }
    };

    // Validate the grant against the live policy snapshot (role vocabulary)
    // before touching the file. A reject leaves the policy file untouched.
    let current = policy_handle.load_full();
    if let Err(reason) = validate_grant(&request.operations, &current) {
        audit_allow(
            audit,
            username,
            args,
            &request.org,
            &request.repo,
            &deny(&reason),
        )
        .await;
        return send_denied(writer, &reason).await;
    }

    let rule = Rule {
        user: request.target_user.clone(),
        org: request.org.clone(),
        repo: request.repo.clone(),
        operations: request.operations.clone(),
        branches: vec!["*".to_string()],
        effect: Effect::Allow,
    };

    // Append + atomic rename happen on a blocking thread; the file work is
    // synchronous and must not stall the async executor under load.
    let path = policy_path.to_path_buf();
    let append_result =
        tokio::task::spawn_blocking(move || append_rule_atomically(&path, &rule)).await;

    let new_text = match append_result {
        Ok(Ok(text)) => text,
        Ok(Err(err)) => {
            let reason = format!(
                "allow: failed to update policy file: {err} (policy path: {}; \
                 hint: set GHBRK_POLICY to a writable path such as /var/etc/ghbrk/policy.yaml)",
                policy_path.display()
            );
            audit_allow(
                audit,
                username,
                args,
                &request.org,
                &request.repo,
                &deny(&reason),
            )
            .await;
            return send_denied(writer, &reason).await;
        }
        Err(err) => {
            let reason = format!("allow: policy write task failed: {err}");
            audit_allow(
                audit,
                username,
                args,
                &request.org,
                &request.repo,
                &deny(&reason),
            )
            .await;
            return send_denied(writer, &reason).await;
        }
    };

    // Reload from the freshly written file so the swapped handle matches disk
    // exactly, then publish it. A parse failure here means the file we just
    // wrote is somehow invalid; surface it rather than swapping in garbage.
    match Policy::from_yaml(&new_text) {
        Ok(reloaded) => policy_handle.store(Arc::new(reloaded)),
        Err(err) => {
            let reason = format!("allow: wrote policy but failed to reload it: {err}");
            audit_allow(
                audit,
                username,
                args,
                &request.org,
                &request.repo,
                &deny(&reason),
            )
            .await;
            return send_denied(writer, &reason).await;
        }
    }

    audit_allow(
        audit,
        username,
        args,
        &request.org,
        &request.repo,
        &AuditDecision::Allow,
    )
    .await;

    let confirmation = format!(
        "granted {} on {}/{} to {}\n",
        describe_operations(&request.operations),
        request.org,
        request.repo,
        request.target_user
    );
    write_frame(
        writer,
        &ServerFrame::StdoutChunk {
            data: confirmation.into_bytes(),
        },
    )
    .await?;
    write_frame(writer, &ServerFrame::Exit { code: 0 }).await
}

/// A parsed allow request: the target repo, the user the grant is for, and the
/// operations spec (a role name or an explicit operation list).
struct AllowRequest {
    org: String,
    repo: String,
    target_user: String,
    operations: OperationsSpec,
}

/// Parse `org/repo`, an optional `--user <name>` flag (defaulting the target to
/// the caller), and the operand list into an [`AllowRequest`]. Returns a
/// human-readable deny reason on any malformed input.
fn parse_allow_args(args: &[String], caller: &str) -> Result<AllowRequest, String> {
    let spec = args
        .first()
        .ok_or_else(|| "allow: missing repo specifier; expected org/repo".to_string())?;
    let (org, repo) = parse_repo_spec(spec)
        .ok_or_else(|| format!("allow: invalid repo specifier '{spec}'; expected org/repo"))?;

    let mut target_user = caller.to_string();
    let mut operands: Vec<&str> = Vec::new();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "--user" {
            let value = rest
                .next()
                .ok_or_else(|| "allow: --user requires a username".to_string())?;
            if value.is_empty() {
                return Err("allow: --user requires a non-empty username".to_string());
            }
            target_user = value.clone();
        } else {
            operands.push(arg.as_str());
        }
    }

    if operands.is_empty() {
        return Err("allow: no operations or role specified".to_string());
    }

    let operations = parse_operations_spec(&operands)?;
    Ok(AllowRequest {
        org,
        repo,
        target_user,
        operations,
    })
}

/// Turn the operand list into an [`OperationsSpec`]. A single operand that
/// names a known role becomes `Role`; otherwise every operand must parse as a
/// concrete operation and the result is a `List`. Returns a deny reason naming
/// the offending operand when it is neither a known role nor a known operation.
fn parse_operations_spec(operands: &[&str]) -> Result<OperationsSpec, String> {
    if operands.len() == 1 {
        let only = operands[0];
        if is_known_role(only) {
            return Ok(OperationsSpec::Role(only.to_string()));
        }
        if let Some(op) = Operation::parse(only) {
            return Ok(OperationsSpec::List(vec![op]));
        }
        return Err(format!(
            "allow: '{only}' is neither a known operation nor a known role"
        ));
    }

    let mut ops = Vec::with_capacity(operands.len());
    for operand in operands {
        match Operation::parse(operand) {
            Some(op) => ops.push(op),
            None => {
                return Err(format!("allow: unknown operation '{operand}'"));
            }
        }
    }
    Ok(OperationsSpec::List(ops))
}

/// True when `name` resolves as a role in the built-in vocabulary. A standalone
/// helper backed by an empty policy: it covers the built-ins (`read-only`,
/// `write`, `admin`) that exist without declaration, which is what the operand
/// classifier needs before the live-policy validation step.
fn is_known_role(name: &str) -> bool {
    Operation::parse(name).is_none() && BUILTIN_ROLE_NAMES.contains(&name)
}

/// Built-in role names available without declaration. Mirrors `policy.rs`.
const BUILTIN_ROLE_NAMES: &[&str] = &["read-only", "write", "admin"];

/// Validate the parsed operations against the live policy. For a role
/// reference, the role must resolve in the current policy (user-defined roles
/// shadow built-ins). For an explicit list, parsing already guaranteed every
/// operation is in the vocabulary.
fn validate_grant(operations: &OperationsSpec, policy: &Policy) -> Result<(), String> {
    match operations {
        OperationsSpec::Role(name) => {
            if policy.resolve_role(name).is_none() {
                return Err(format!("allow: unknown role '{name}'"));
            }
            Ok(())
        }
        OperationsSpec::List(_) => Ok(()),
    }
}

/// Read the current policy file, append `rule`, and write the re-serialised
/// document back atomically via a temp file in the same directory plus a
/// rename. Returns the serialised text on success. Never partially writes the
/// destination: a crash mid-write leaves the original file intact.
fn append_rule_atomically(policy_path: &Path, rule: &Rule) -> io::Result<String> {
    let text = std::fs::read_to_string(policy_path)?;
    let mut policy =
        Policy::from_yaml(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    policy.rules.push(rule.clone());
    let serialised = serde_yaml::to_string(&policy)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let dir = policy_path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::Builder::new()
        .prefix(".policy-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    {
        use std::io::Write as _;
        tmp.as_file_mut()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        tmp.write_all(serialised.as_bytes())?;
        tmp.as_file_mut().sync_all()?;
    }
    // Atomic replace on the same filesystem.
    tmp.persist(policy_path)
        .map_err(|e| io::Error::other(e.error))?;
    Ok(serialised)
}

/// Human-readable summary of an operations spec for the confirmation line.
fn describe_operations(operations: &OperationsSpec) -> String {
    match operations {
        OperationsSpec::Role(name) => format!("role '{name}'"),
        OperationsSpec::List(ops) => {
            let names: Vec<&str> = ops.iter().map(operation_name).collect();
            names.join(", ")
        }
    }
}

fn deny(reason: &str) -> AuditDecision {
    AuditDecision::Deny {
        reason: reason.to_string(),
    }
}

/// Write an audit record for an allow request outcome.
async fn audit_allow(
    audit: &Arc<AuditLogger>,
    user: &str,
    args: &[String],
    org: &str,
    repo: &str,
    decision: &AuditDecision,
) {
    write_audit(
        audit,
        AuditEntry {
            user,
            tool: "allow",
            args,
            org,
            repo,
            branch: None,
            operation: "allow",
            decision: decision.clone(),
        },
    )
    .await;
}

/// Returns `true` when a `gh` invocation is a broker-mediated operation
/// (subject to resolve + policy). When `false`, the broker treats it as a
/// passthrough: it still injects `GH_TOKEN` but bypasses resolve and policy.
fn gh_is_broker_op(args: &[String]) -> bool {
    let mut positional = args.iter().filter(|a| !a.starts_with('-'));
    let group = positional.next().map(String::as_str).unwrap_or("");
    let action = positional.next().map(String::as_str).unwrap_or("");
    if group == "api" {
        return true;
    }
    matches!(
        (group, action),
        ("pr", "create")
            | ("pr", "comment")
            | ("pr", "merge")
            | ("pr", "close")
            | ("pr", "review")
            | ("issue", "create")
            | ("issue", "comment")
            | ("issue", "close")
            | ("release", "create")
    )
}

async fn handle_gh_passthrough(
    stream: &mut UnixStream,
    request: &Request,
    identity: &PeerIdentity,
    audit: &Arc<AuditLogger>,
    credentials_root: Option<&Path>,
) -> Result<(), ConnectionError> {
    let username = identity.username.as_str();
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
        uid: Some(identity.uid),
        gid: Some(identity.gid),
        supplementary_gids: identity.supplementary_gids.clone(),
        home: Some(identity.home.clone()),
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

    // Report observed owner/mode of the credential dir and files. The caller
    // cannot stat these paths (broker-owned, mode 0700), so it relies on the
    // broker to proxy the stat and runs the permission classifier client-side.
    let audit = crate::health_check::audit_credential_paths(&creds_root, username);
    write_frame(stream, &ServerFrame::CredentialAudit { audit }).await?;

    let code = if all_ok { 0 } else { 1 };
    write_frame(stream, &ServerFrame::Exit { code }).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// Every operation in the policy vocabulary, used by [`handle_policy_request`]
/// to report the caller's full allow/deny surface against a repo.
const ALL_OPS: &[Operation] = &[
    Operation::Push,
    Operation::Fetch,
    Operation::Pull,
    Operation::Clone,
    Operation::PrOpen,
    Operation::PrComment,
    Operation::PrClose,
    Operation::PrMerge,
    Operation::PrReview,
    Operation::IssueOpen,
    Operation::IssueComment,
    Operation::IssueClose,
    Operation::ReleaseCreate,
    Operation::GhApiRead {
        path: String::new(),
    },
];

/// A git invocation leaves the machine only for `push`, `fetch`, `clone`, and
/// `pull`. Everything else (including an empty argv) is local-only. Mirrors the
/// client-side gateway filter in `cmd/git.rs`.
fn git_is_remote_op(args: &[String]) -> bool {
    matches!(
        git_first_subcommand(args),
        Some("push" | "fetch" | "clone" | "pull")
    )
}

/// First non-flag positional argument, skipping git global flags that take a
/// value (`-c`, `-C`, `--config`, `--git-dir`, `--work-tree`, `--namespace`).
fn git_first_subcommand(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') {
            return Some(arg.as_str());
        }
        if matches!(
            arg.as_str(),
            "-c" | "-C" | "--config" | "--git-dir" | "--work-tree" | "--namespace"
        ) {
            iter.next();
        }
    }
    None
}

/// Resolve + evaluate a request without executing it, streaming a human
/// readable explanation back to the client. Never spawns a subprocess.
async fn handle_explain_request(
    stream: &mut UnixStream,
    request: &Request,
    username: &str,
    policy: &Policy,
) -> Result<(), ConnectionError> {
    let tool = request.args.first().map(String::as_str).unwrap_or("");
    match tool {
        "" => {
            stream_text(stream, "explain: no command provided\n").await?;
            return finish_explain(stream, 1).await;
        }
        "git" => {
            let sub = &request.args[1..];
            if !git_is_remote_op(sub) {
                let subcmd = git_first_subcommand(sub).unwrap_or("");
                stream_text(stream, &explain_local_git(subcmd)).await?;
                return finish_explain(stream, 0).await;
            }
            explain_resolved(stream, request, username, policy, Tool::Git, "git").await
        }
        "gh" => explain_resolved(stream, request, username, policy, Tool::Gh, "gh").await,
        other => {
            stream_text(
                stream,
                &format!("explain: unknown tool '{other}'; expected 'git' or 'gh'\n"),
            )
            .await?;
            finish_explain(stream, 1).await
        }
    }
}

/// Resolve `request.args[1..]` for the named tool, evaluate policy, and stream
/// the resolved-and-evaluated report. On resolver error, streams the error and
/// exits with code 1.
async fn explain_resolved(
    stream: &mut UnixStream,
    request: &Request,
    username: &str,
    policy: &Policy,
    tool: Tool,
    tool_name: &str,
) -> Result<(), ConnectionError> {
    let sub = &request.args[1..];
    let resolved = match resolve_for_tool(
        tool,
        sub,
        &request.cwd,
        request.remote_url.as_deref(),
        request.head_branch.as_deref(),
    ) {
        Ok(r) => r,
        Err(err) => {
            stream_text(stream, &format!("explain: resolver error: {err}\n")).await?;
            return finish_explain(stream, 1).await;
        }
    };

    let policy_req = PolicyRequest {
        user: username,
        org: &resolved.org,
        repo: &resolved.repo,
        operation: resolved.operation.clone(),
        branch: resolved.branch.as_deref(),
    };
    let decision = policy.evaluate(&policy_req);
    let report = explain_report(tool, tool_name, sub, &resolved, &decision);
    stream_text(stream, &report).await?;
    finish_explain(stream, 0).await
}

fn resolve_for_tool(
    tool: Tool,
    args: &[String],
    cwd: &Path,
    url_hint: Option<&str>,
    branch_hint: Option<&str>,
) -> Result<ResolvedRequest, ResolverError> {
    match tool {
        Tool::Git => resolve_git(args, cwd, url_hint, branch_hint),
        Tool::Gh => resolve_gh(args, cwd, url_hint, branch_hint),
        _ => unreachable!("resolve_for_tool only handles Git and Gh"),
    }
}

fn explain_local_git(subcmd: &str) -> String {
    format!(
        "tool:      git {subcmd}\n\
         scope:     local — outside ghbrk's gateway\n\
         guidance:  run 'git {subcmd}' directly; ghbrk only brokers remote git operations (push, fetch, clone, pull)\n\
         policy:    N/A (not evaluated)\n\
         inject:    none\n"
    )
}

fn explain_report(
    tool: Tool,
    tool_name: &str,
    sub: &[String],
    resolved: &ResolvedRequest,
    decision: &Decision,
) -> String {
    let subcmd = sub.first().map(String::as_str).unwrap_or("");
    let op = operation_name(&resolved.operation);
    let repo = format!("{}/{}", resolved.org, resolved.repo);
    let branch = resolved.branch.as_deref().unwrap_or("N/A");
    let (policy_line, inject) = match decision {
        Decision::Allow => ("allow".to_string(), inject_label(tool, resolved.url_scheme)),
        Decision::Deny { reason } => (format!("deny: {reason}"), "none"),
    };
    format!(
        "tool:      {tool_name} {subcmd}\n\
         operation: {op}\n\
         repo:      {repo}\n\
         branch:    {branch}\n\
         policy:    {policy_line}\n\
         inject:    {inject}\n"
    )
}

fn inject_label(tool: Tool, scheme: UrlScheme) -> &'static str {
    match tool {
        Tool::Gh => "GitHub token (GH_TOKEN)",
        Tool::Git => match scheme {
            UrlScheme::Ssh => "SSH credential",
            UrlScheme::Https => "HTTPS credential (GIT_ASKPASS)",
        },
        _ => "none",
    }
}

/// Evaluate every operation in the vocabulary for the caller against the
/// requested repo and stream a grouped allowed/forbidden report. Never spawns
/// a subprocess.
async fn handle_policy_request(
    stream: &mut UnixStream,
    request: &Request,
    username: &str,
    policy: &Policy,
) -> Result<(), ConnectionError> {
    let spec = request.args.first().map(String::as_str).unwrap_or("");
    let (org, repo) = match parse_repo_spec(spec) {
        Some(pair) => pair,
        None => {
            stream_text(
                stream,
                &format!("policy: invalid repo specifier '{spec}'; expected org/repo\n"),
            )
            .await?;
            return finish_explain(stream, 1).await;
        }
    };

    let mut allowed: Vec<&str> = Vec::new();
    let mut forbidden: Vec<&str> = Vec::new();
    for op in ALL_OPS {
        let req = PolicyRequest {
            user: username,
            org: &org,
            repo: &repo,
            operation: op.clone(),
            branch: None,
        };
        match policy.evaluate(&req) {
            Decision::Allow => allowed.push(operation_name(op)),
            Decision::Deny { .. } => forbidden.push(operation_name(op)),
        }
    }

    let report = policy_report(&org, &repo, &allowed, &forbidden);
    stream_text(stream, &report).await?;
    finish_explain(stream, 0).await
}

fn parse_repo_spec(spec: &str) -> Option<(String, String)> {
    let (org, repo) = spec.split_once('/')?;
    if org.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((org.to_string(), repo.to_string()))
}

fn policy_report(org: &str, repo: &str, allowed: &[&str], forbidden: &[&str]) -> String {
    let mut out = format!("repo: {org}/{repo}\n\nallowed operations:\n");
    if allowed.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for op in allowed {
            out.push_str(&format!("  {op}\n"));
        }
    }
    out.push_str("\nforbidden operations (default-deny):\n");
    if forbidden.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for op in forbidden {
            out.push_str(&format!("  {op}\n"));
        }
    }
    out
}

async fn stream_text(stream: &mut UnixStream, text: &str) -> Result<(), ConnectionError> {
    write_frame(
        stream,
        &ServerFrame::StdoutChunk {
            data: text.as_bytes().to_vec(),
        },
    )
    .await?;
    Ok(())
}

async fn finish_explain(stream: &mut UnixStream, code: i32) -> Result<(), ConnectionError> {
    write_frame(stream, &ServerFrame::Exit { code }).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn resolve_request(request: &Request) -> Result<ResolvedRequest, ResolverError> {
    match request.tool {
        Tool::Git => resolve_git(
            &request.args,
            &request.cwd,
            request.remote_url.as_deref(),
            request.head_branch.as_deref(),
        ),
        Tool::Gh => resolve_gh(
            &request.args,
            &request.cwd,
            request.remote_url.as_deref(),
            request.head_branch.as_deref(),
        ),
        Tool::Check => unreachable!("Tool::Check is handled before resolve_request"),
        Tool::Explain | Tool::Policy => {
            unreachable!("query tools are handled before resolve_request")
        }
        Tool::Allow => unreachable!("Tool::Allow is handled before resolve_request"),
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

/// Env var bindings plus optional RAII keep-alive guards (HTTPS askpass script, SSH agent).
type BuiltEnv = (
    Vec<(String, String)>,
    Option<crate::credentials::HttpsGitEnv>,
    Option<SshAgentHandle>,
);

/// Build the env-var pairs for the chosen tool and URL scheme. Returns the
/// vars plus optional keep-alive RAII guards (the HTTPS askpass script and the
/// SSH agent escrow).
async fn build_env(
    tool: &Tool,
    resolved: &ResolvedRequest,
    creds: &Credentials,
) -> Result<BuiltEnv, CredentialError> {
    match tool {
        Tool::Gh => Ok((gh_env(creds), None, None)),
        Tool::Git => match resolved.url_scheme {
            UrlScheme::Ssh => {
                let (env, handle) = start_ssh_agent(creds).await?;
                Ok((env, None, Some(handle)))
            }
            UrlScheme::Https => {
                let env = https_git_env(creds)?;
                let vars = env.vars.clone();
                Ok((vars, Some(env), None))
            }
        },
        Tool::Check => unreachable!("Tool::Check is handled before build_env"),
        Tool::Explain | Tool::Policy => unreachable!("query tools are handled before build_env"),
        Tool::Allow => unreachable!("Tool::Allow is handled before build_env"),
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

    #[tokio::test]
    async fn peer_identity_resolves_current_process() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        a.set_nonblocking(true).expect("nonblocking");
        let stream = UnixStream::from_std(a).expect("tokio adoption");
        let identity = peer_identity(&stream).expect("current process must resolve");
        assert_eq!(identity.uid, nix::unistd::geteuid().as_raw());
        assert!(!identity.username.is_empty());
        assert!(!identity.home.as_os_str().is_empty());
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

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn gh_api_is_broker_op() {
        assert!(gh_is_broker_op(&s(&["api", "user"])));
    }

    #[test]
    fn gh_write_ops_are_broker_ops() {
        assert!(gh_is_broker_op(&s(&["pr", "create", "--title", "x"])));
        assert!(gh_is_broker_op(&s(&[
            "pr", "comment", "42", "--body", "hi"
        ])));
        assert!(gh_is_broker_op(&s(&["pr", "merge", "42"])));
        assert!(gh_is_broker_op(&s(&["pr", "close", "42"])));
        assert!(gh_is_broker_op(&s(&["pr", "review", "42", "--approve"])));
        assert!(gh_is_broker_op(&s(&["issue", "create", "--title", "bug"])));
        assert!(gh_is_broker_op(&s(&[
            "issue", "comment", "1", "--body", "x"
        ])));
        assert!(gh_is_broker_op(&s(&["issue", "close", "1"])));
        assert!(gh_is_broker_op(&s(&["release", "create", "v1.0.0"])));
    }

    #[test]
    fn gh_read_ops_are_passthrough() {
        assert!(!gh_is_broker_op(&s(&["auth", "status"])));
        assert!(!gh_is_broker_op(&s(&["repo", "view"])));
        assert!(!gh_is_broker_op(&s(&["pr", "list"])));
        assert!(!gh_is_broker_op(&s(&["pr", "frobnicate"])));
        assert!(!gh_is_broker_op(&s(&[])));
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
