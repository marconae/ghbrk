use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tokio::net::UnixStream;

use ghbrk::health_check::SHARED_FILESYSTEM_LABEL;
use ghbrk::policy::Policy;
use ghbrk::protocol::{
    read_frame, write_frame, CredentialAudit, PathAudit, Request, ServerFrame, TmpIdentity, Tool,
};

use super::gateway::socket_path_from_env;

/// Default policy file path, overridable via `GHBRK_POLICY`.
const DEFAULT_POLICY_PATH: &str = "/etc/ghbrk/policy.yaml";

/// Default config directory, used when the policy path has no parent component.
const DEFAULT_CONFIG_DIR: &str = "/etc/ghbrk";

/// Environment variable that overrides the default policy file path.
const POLICY_ENV_VAR: &str = "GHBRK_POLICY";

pub fn run() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("ghbrk: failed to start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    let socket_path = socket_path_from_env();
    let policy_path = policy_path_from_env();

    let config_dir = config_dir_from_policy_path(&policy_path);

    let caller_tmp = match caller_tmp_identity() {
        Ok(identity) => Some(identity),
        Err(err) => {
            println!("{SHARED_FILESYSTEM_LABEL} SKIPPED (cannot stat own /tmp: {err})");
            None
        }
    };
    let (daemon_ok, checks) = runtime.block_on(check_daemon_and_creds(&socket_path, caller_tmp));
    let policy_verdict = check_policy(&policy_path);

    // Collect a verdict from every check before it decides the exit code.
    // Each check above already printed its own status line. No check stops
    // early because of an earlier failure.
    let mut verdicts = vec![
        verdict_from_success(daemon_ok, "daemon unreachable"),
        verdict_from_success(checks.credentials_ok, "credential check failed"),
        checks.shared_filesystem,
        policy_verdict,
        check_policy_permissions(&policy_path),
        check_config_dir_permissions(&config_dir),
        check_socket_permissions(&socket_path),
    ];
    verdicts.extend(checks.credential_paths);

    aggregate_exit(&verdicts)
}

/// Map a boolean check outcome onto the tiered verdict scale so it can join the
/// aggregate: success is `Ok`, failure is an `Error` carrying `failure_detail`.
fn verdict_from_success(success: bool, failure_detail: &str) -> PermissionVerdict {
    if success {
        PermissionVerdict::Ok
    } else {
        PermissionVerdict::Error(failure_detail.to_string())
    }
}

/// Fold every check's verdict into one exit code. The command fails when at
/// least one verdict is `Error`. A `Warning` verdict never changes the exit
/// status, and an empty set of verdicts is a success.
fn aggregate_exit(verdicts: &[PermissionVerdict]) -> ExitCode {
    let has_error = verdicts
        .iter()
        .any(|verdict| matches!(verdict, PermissionVerdict::Error(_)));
    if has_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn policy_path_from_env() -> PathBuf {
    env::var_os(POLICY_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_POLICY_PATH))
}

/// The config directory is the directory holding the policy file, so it
/// tracks the `GHBRK_POLICY` override seam. It falls back to `/etc/ghbrk`
/// when the policy path is bare.
fn config_dir_from_policy_path(policy_path: &Path) -> PathBuf {
    match policy_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from(DEFAULT_CONFIG_DIR),
    }
}

/// Stats the caller's own `/tmp` for the `cli/doctor` shared-filesystem
/// check. It creates no file and prints nothing. It leaves the report of a
/// stat failure to the caller: a caller that cannot stat its own `/tmp` has
/// nothing to compare, and that absence is not a fault the broker can
/// diagnose.
fn caller_tmp_identity() -> io::Result<TmpIdentity> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::metadata("/tmp")?;
    Ok(TmpIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

/// Outcome of one `check` request: one verdict per subsystem it covers.
/// The broker folds every check into one exit code. A single verdict
/// cannot say which subsystem failed, so this struct keeps one verdict per
/// subsystem. Each verdict reaches the exit decision on its own, and each
/// failure is reported under its own heading.
struct CheckVerdicts {
    credentials_ok: bool,
    shared_filesystem: PermissionVerdict,
    credential_paths: Vec<PermissionVerdict>,
}

impl CheckVerdicts {
    /// The verdicts for a check request whose credential result is unresolved:
    /// the daemon was unreachable, the transport failed, or the broker denied
    /// the request. The caller already printed the reason under the
    /// `Credentials:` heading. The shared-filesystem check never ran, so it
    /// does not affect the exit decision.
    fn credentials_unresolved(credential_paths: Vec<PermissionVerdict>) -> Self {
        Self {
            credentials_ok: false,
            shared_filesystem: PermissionVerdict::Ok,
            credential_paths,
        }
    }
}

/// Attempt to connect to the broker socket and run the Check tool.
///
/// Returns `(daemon_ok, verdicts)`. See [`CheckVerdicts`] for why the checks
/// the broker ran are reported per subsystem rather than as one boolean.
async fn check_daemon_and_creds(
    socket_path: &Path,
    caller_tmp: Option<TmpIdentity>,
) -> (bool, CheckVerdicts) {
    match UnixStream::connect(socket_path).await {
        Err(err) => {
            println!("Daemon: UNREACHABLE ({}: {})", socket_path.display(), err);
            println!("Credentials: SKIPPED (daemon unreachable)");
            (false, CheckVerdicts::credentials_unresolved(vec![]))
        }
        Ok(stream) => {
            println!("Daemon: OK");
            (true, run_check_via_broker(stream, caller_tmp).await)
        }
    }
}

/// Send a `Tool::Check` request via an already-connected stream, print one
/// heading per subsystem the broker reported on, and return their verdicts.
///
/// The broker's exit code names no subsystem. It is the combined result of
/// every check the broker ran, so it is not the credentials verdict on its
/// own. The credentials verdict comes from the credential status lines and
/// the path audit. See [`credentials_passed`].
async fn run_check_via_broker(
    stream: UnixStream,
    caller_tmp: Option<TmpIdentity>,
) -> CheckVerdicts {
    let cwd = env::current_dir().unwrap_or_default();
    let request = Request {
        tool: Tool::Check,
        args: vec![],
        cwd,
        remote_url: None,
        head_branch: None,
        client_frames: false,
        caller_tmp,
    };

    let (read_half, mut write_half) = stream.into_split();
    if write_frame(&mut write_half, &request).await.is_err() {
        println!("Credentials: FAILED");
        return CheckVerdicts::credentials_unresolved(vec![]);
    }

    let mut reader = read_half;
    let mut broker_output = Vec::<u8>::new();
    let mut cred_verdicts = Vec::new();
    let exit_code = loop {
        match read_frame::<_, ServerFrame>(&mut reader).await {
            Ok(ServerFrame::StdoutChunk { data }) => broker_output.extend_from_slice(&data),
            Ok(ServerFrame::StderrChunk { data }) => broker_output.extend_from_slice(&data),
            Ok(ServerFrame::CredentialAudit { audit }) => {
                cred_verdicts = classify_credential_audit(&audit);
            }
            Ok(ServerFrame::Exit { code }) => break code,
            Ok(ServerFrame::Denied { reason }) => {
                println!("Credentials: FAILED");
                println!("{reason}");
                return CheckVerdicts::credentials_unresolved(cred_verdicts);
            }
            Err(_) => {
                println!("Credentials: FAILED");
                return CheckVerdicts::credentials_unresolved(cred_verdicts);
            }
        }
    };

    let broker_output = String::from_utf8_lossy(&broker_output).into_owned();
    let (shared_fs_line, credential_output) = split_out_shared_filesystem_line(&broker_output);

    let credentials_ok = credentials_passed(exit_code, &credential_output, &cred_verdicts);
    if credentials_ok {
        println!("Credentials: OK");
    } else {
        println!("Credentials: FAILED");
        if !credential_output.is_empty() {
            print!("{credential_output}");
        }
    }

    let shared_filesystem = match shared_fs_line {
        Some(line) => {
            println!("{line}");
            shared_filesystem_verdict(&line)
        }
        None if caller_tmp.is_some() => {
            println!("{SHARED_FILESYSTEM_LABEL} UNSUPPORTED (daemon did not report this check)");
            PermissionVerdict::Warning("daemon reported no shared-filesystem check".to_string())
        }
        None => PermissionVerdict::Ok,
    };

    CheckVerdicts {
        credentials_ok,
        shared_filesystem,
        credential_paths: cred_verdicts,
    }
}

/// Verdict token a broker status line carries when its check passed.
const STATUS_PASS: &str = "OK";

/// Verdict token a broker status line carries when its check did not run and
/// that absence is not itself a fault.
const STATUS_NOT_RUN: &str = "SKIPPED";

/// Answers whether one `<label>: <VERDICT> [(detail)]` line of the broker's
/// check output reports a pass.
///
/// Recognition is positive: only the pass and not-run tokens count. A
/// failure token is never read as a pass. Nor is any line shape this client
/// does not know, including one a newer daemon adds later.
fn status_line_passes(line: &str) -> bool {
    match line.split_once(": ") {
        Some((_, verdict)) => {
            let token = verdict.split_whitespace().next().unwrap_or_default();
            token == STATUS_PASS || token == STATUS_NOT_RUN
        }
        None => false,
    }
}

/// Decides the `Credentials:` verdict for a check request that ran to
/// completion.
///
/// The broker's exit code is the result of every check it ran, the
/// shared-filesystem check included, so it cannot name the failing
/// subsystem. Read alone, it marks a mismatched `/tmp` as a credential
/// failure. A zero exit is still conclusive: the broker passed every check
/// it ran. Otherwise the credential status lines and the path audit
/// decide, and an unrecognized line fails closed.
fn credentials_passed(
    exit_code: i32,
    credential_output: &str,
    path_verdicts: &[PermissionVerdict],
) -> bool {
    let every_line_passes = credential_output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(status_line_passes);
    let no_path_error = !path_verdicts
        .iter()
        .any(|verdict| matches!(verdict, PermissionVerdict::Error(_)));

    (exit_code == 0 || every_line_passes) && no_path_error
}

/// Classifies the broker's `Shared filesystem:` status line onto the tiered
/// verdict scale so it reaches the doctor aggregate as its own signal rather
/// than through the credentials verdict.
fn shared_filesystem_verdict(line: &str) -> PermissionVerdict {
    if status_line_passes(line) {
        PermissionVerdict::Ok
    } else {
        PermissionVerdict::Error(line.to_string())
    }
}

/// Pulls the line starting `Shared filesystem:` out of the broker's
/// collected check output, if present. This lets the caller print it
/// unconditionally, not only when the overall check fails, and lets the
/// credentials verdict come from the credential lines alone. Returns the
/// extracted line, without its trailing newline, and the remaining output
/// with that line removed.
fn split_out_shared_filesystem_line(output: &str) -> (Option<String>, String) {
    let mut shared_fs_line = None;
    let mut remaining = String::new();
    for line in output.lines() {
        if line.starts_with(SHARED_FILESYSTEM_LABEL) {
            shared_fs_line = Some(line.to_string());
        } else {
            remaining.push_str(line);
            remaining.push('\n');
        }
    }
    (shared_fs_line, remaining)
}

/// Tiered verdict for a permission audit check: a write-path exposure is an
/// `Error`, a read-path widening is a `Warning`, and a correctly locked-down
/// path is `Ok`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionVerdict {
    Ok,
    Warning(String),
    Error(String),
}

/// The kind of filesystem object being audited. The write-path and
/// read-path exposure rules differ per kind. A directory's execute bit is
/// traversal, a write-path concern. A socket cares only about a non-group
/// connect access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathKind {
    File,
    Directory,
    Socket,
}

/// Permission bits writable by group or other (`0o022`).
const GROUP_OTHER_WRITE: u32 = 0o022;

/// Permission bits readable by group or other (`0o044`).
const GROUP_OTHER_READ: u32 = 0o044;

/// Group/other write **or** execute bits (`0o033`). On a directory these are all
/// write-path: traversal lets a non-owner replace the directory's children.
const GROUP_OTHER_WRITE_EXEC: u32 = 0o033;

/// Other read/write bits (`0o006`). A socket with any of these lets a user
/// outside the owning group connect and issue broker requests.
const OTHER_CONNECT: u32 = 0o006;

/// All permission bits (owner/group/other rwx), used to strip file-type bits
/// from a raw `st_mode`.
const PERMISSION_BITS: u32 = 0o777;

/// Mode the policy file must have: readable and writable by the owner only.
const POLICY_EXPECTED_MODE: u32 = 0o600;

/// The system user that must own the policy file.
const POLICY_OWNER_USER: &str = "ghbrk";

/// The system user that must own the broker socket.
const SOCKET_OWNER_USER: &str = "ghbrk";

/// Uid the config directory must be owned by: `root`.
const ROOT_UID: u32 = 0;

/// Mode the config directory must have: owner-writable, world-traversable.
const CONFIG_DIR_EXPECTED_MODE: u32 = 0o755;

/// Mode the broker socket must have: owner and group read/write only.
const SOCKET_EXPECTED_MODE: u32 = 0o660;

/// The system user that must own the credential directory and credential files.
const CREDENTIAL_OWNER_USER: &str = "ghbrk";

/// Mode the per-user credential directory must have: owner rwx only.
const CREDENTIAL_DIR_EXPECTED_MODE: u32 = 0o700;

/// Mode each credential file must have: owner read/write only.
const CREDENTIAL_FILE_EXPECTED_MODE: u32 = 0o600;

/// Pure, path-kind-aware classifier mapping an observed owner uid and `st_mode`
/// onto the tiered verdict scale. A wrong owner is always a write-path exposure
/// (`Error`) and dominates the mode rules. Otherwise the exposure tier depends
/// on the path kind:
///
/// - File: group/other **write** is `Error`; group/other **read** is `Warning`.
/// - Directory: group/other **write or execute** is `Error` (traversal lets a
///   non-owner replace children); group/other **read** is `Warning`.
/// - Socket: any **other** read/write bit is `Error`. A non-group user can
///   connect. Group access is intended, so there is no read-path widening.
///
/// Anything with no exposure is `Ok`. The function masks the raw `st_mode` to
/// its permission bits internally, so callers can pass the value from
/// `metadata` directly.
fn classify_permissions(
    path_kind: PathKind,
    expected_owner_uid: u32,
    observed_owner_uid: u32,
    expected_mode: u32,
    observed_mode: u32,
) -> PermissionVerdict {
    if observed_owner_uid != expected_owner_uid {
        return PermissionVerdict::Error(format!(
            "owner uid {observed_owner_uid} (expected {expected_owner_uid})"
        ));
    }

    let perms = observed_mode & PERMISSION_BITS;
    let expected = expected_mode & PERMISSION_BITS;

    // Only bits granted *beyond* the expected mode are an exposure. This lets a
    // legitimately world-traversable config directory (`0755`) pass while still
    // flagging any access widened past its expected mode.
    let excess = perms & !expected;

    match path_kind {
        PathKind::File => {
            if excess & GROUP_OTHER_WRITE != 0 {
                return PermissionVerdict::Error(format!(
                    "mode {perms:#o} is group/other-writable (expected {expected:#o})"
                ));
            }
            if excess & GROUP_OTHER_READ != 0 {
                return PermissionVerdict::Warning(format!(
                    "mode {perms:#o} is group/other-readable (expected {expected:#o})"
                ));
            }
        }
        PathKind::Directory => {
            if excess & GROUP_OTHER_WRITE_EXEC != 0 {
                return PermissionVerdict::Error(format!(
                    "mode {perms:#o} is group/other-writable or -traversable (expected {expected:#o})"
                ));
            }
            if excess & GROUP_OTHER_READ != 0 {
                return PermissionVerdict::Warning(format!(
                    "mode {perms:#o} is group/other-readable (expected {expected:#o})"
                ));
            }
        }
        PathKind::Socket => {
            if excess & OTHER_CONNECT != 0 {
                return PermissionVerdict::Error(format!(
                    "mode {perms:#o} permits non-group connection (expected {expected:#o})"
                ));
            }
        }
    }

    PermissionVerdict::Ok
}

/// Pure predicate classifying the policy file's owner and mode onto the
/// tiered verdict scale. It calls the generic [`classify_permissions`]
/// classifier: the policy file is a regular file, owned by `ghbrk`,
/// expected at mode `0600`.
fn classify_policy_permissions(
    observed_uid: u32,
    expected_uid: u32,
    mode: u32,
) -> PermissionVerdict {
    classify_permissions(
        PathKind::File,
        expected_uid,
        observed_uid,
        POLICY_EXPECTED_MODE,
        mode,
    )
}

/// Stat the policy file, resolve the expected owner uid, classify owner+mode,
/// and print a single `Policy permissions:` status line. Returns the verdict so
/// the doctor aggregate can fold it into the overall exit code.
fn check_policy_permissions(path: &Path) -> PermissionVerdict {
    use std::os::unix::fs::MetadataExt;

    let expected_uid = match nix::unistd::User::from_name(POLICY_OWNER_USER) {
        Ok(Some(user)) => user.uid.as_raw(),
        Ok(None) => {
            let verdict = PermissionVerdict::Warning(format!(
                "user {POLICY_OWNER_USER} not found; cannot verify owner"
            ));
            print_permission_verdict("Policy permissions", &verdict);
            return verdict;
        }
        Err(err) => {
            let verdict = PermissionVerdict::Warning(format!(
                "looking up user {POLICY_OWNER_USER} failed: {err}"
            ));
            print_permission_verdict("Policy permissions", &verdict);
            return verdict;
        }
    };

    let verdict = match std::fs::metadata(path) {
        Ok(meta) => classify_policy_permissions(meta.uid(), expected_uid, meta.mode()),
        Err(err) => PermissionVerdict::Error(format!("cannot stat {}: {err}", path.display())),
    };
    print_permission_verdict("Policy permissions", &verdict);
    verdict
}

/// Print a single `<label>: OK|WARNING <detail>|ERROR <detail>` status line.
fn print_permission_verdict(label: &str, verdict: &PermissionVerdict) {
    match verdict {
        PermissionVerdict::Ok => println!("{label}: OK"),
        PermissionVerdict::Warning(detail) => println!("{label}: WARNING {detail}"),
        PermissionVerdict::Error(detail) => println!("{label}: ERROR {detail}"),
    }
}

/// Classify a single [`PathAudit`] entry against the expected owner uid and
/// permission mode. An absent path is always an `Error`.
fn classify_path_audit(
    entry: &PathAudit,
    kind: PathKind,
    expected_uid: u32,
    expected_mode: u32,
) -> PermissionVerdict {
    if !entry.present {
        return PermissionVerdict::Error("not found".to_string());
    }
    classify_permissions(
        kind,
        expected_uid,
        entry.observed_owner_uid,
        expected_mode,
        entry.observed_mode,
    )
}

/// Run the tiered permission classifier over all entries in a
/// [`CredentialAudit`] frame and return one verdict per entry.
///
/// The first entry is treated as a directory. The rest are treated as
/// regular files. This prints a `<label> permissions: OK|WARNING|ERROR`
/// status line for every entry. If the `ghbrk` owner uid cannot be
/// resolved, it also prints one additional line.
fn classify_credential_audit(audit: &CredentialAudit) -> Vec<PermissionVerdict> {
    let expected_uid = match resolve_owner_uid(CREDENTIAL_OWNER_USER) {
        Ok(uid) => uid,
        Err(verdict) => {
            print_permission_verdict("Credential dir permissions", &verdict);
            return vec![verdict];
        }
    };

    let mut verdicts = Vec::with_capacity(audit.entries.len());
    let mut iter = audit.entries.iter();

    if let Some(dir_entry) = iter.next() {
        let v = classify_path_audit(
            dir_entry,
            PathKind::Directory,
            expected_uid,
            CREDENTIAL_DIR_EXPECTED_MODE,
        );
        print_permission_verdict(&format!("{} permissions", dir_entry.label), &v);
        verdicts.push(v);
    }

    for file_entry in iter {
        let v = classify_path_audit(
            file_entry,
            PathKind::File,
            expected_uid,
            CREDENTIAL_FILE_EXPECTED_MODE,
        );
        print_permission_verdict(&format!("{} permissions", file_entry.label), &v);
        verdicts.push(v);
    }

    verdicts
}

/// Resolve a system user name to its uid. Maps the not-found and
/// lookup-error cases onto a `Warning` verdict: the owner cannot be
/// verified, but the absence of the service account is not a write-path
/// exposure.
fn resolve_owner_uid(user: &str) -> Result<u32, PermissionVerdict> {
    match nix::unistd::User::from_name(user) {
        Ok(Some(u)) => Ok(u.uid.as_raw()),
        Ok(None) => Err(PermissionVerdict::Warning(format!(
            "user {user} not found; cannot verify owner"
        ))),
        Err(err) => Err(PermissionVerdict::Warning(format!(
            "looking up user {user} failed: {err}"
        ))),
    }
}

/// Stat the config directory, classify owner+mode against `root`/`0755`, print a
/// `Config dir permissions:` status line, and return the verdict.
fn check_config_dir_permissions(path: &Path) -> PermissionVerdict {
    use std::os::unix::fs::MetadataExt;

    let verdict = match std::fs::metadata(path) {
        Ok(meta) => classify_permissions(
            PathKind::Directory,
            ROOT_UID,
            meta.uid(),
            CONFIG_DIR_EXPECTED_MODE,
            meta.mode(),
        ),
        Err(err) => PermissionVerdict::Error(format!("cannot stat {}: {err}", path.display())),
    };
    print_permission_verdict("Config dir permissions", &verdict);
    verdict
}

/// Stat the broker socket, classify owner+mode against `ghbrk`/`0660`, print a
/// `Socket permissions:` status line, and return the verdict.
fn check_socket_permissions(path: &Path) -> PermissionVerdict {
    use std::os::unix::fs::MetadataExt;

    let expected_uid = match resolve_owner_uid(SOCKET_OWNER_USER) {
        Ok(uid) => uid,
        Err(verdict) => {
            print_permission_verdict("Socket permissions", &verdict);
            return verdict;
        }
    };

    let verdict = match std::fs::metadata(path) {
        Ok(meta) => classify_permissions(
            PathKind::Socket,
            expected_uid,
            meta.uid(),
            SOCKET_EXPECTED_MODE,
            meta.mode(),
        ),
        Err(err) => PermissionVerdict::Error(format!("cannot stat {}: {err}", path.display())),
    };
    print_permission_verdict("Socket permissions", &verdict);
    verdict
}

/// Compute the policy file status as a `(verdict, message)` pair without
/// printing. This pure helper enables unit-testing the message content.
fn policy_status(path: &Path) -> (PermissionVerdict, String) {
    match std::fs::File::open(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => (
            PermissionVerdict::Error("missing".to_string()),
            format!("Policy: MISSING ({})", path.display()),
        ),
        Err(ref err) if err.kind() == io::ErrorKind::PermissionDenied => {
            (PermissionVerdict::Ok, String::new())
        }
        Err(err) => (
            PermissionVerdict::Error(err.to_string()),
            format!("Policy: ERROR ({}: {})", path.display(), err),
        ),
        Ok(file) => match Policy::from_reader(file) {
            Ok(_) => (PermissionVerdict::Ok, "Policy: OK".to_string()),
            Err(err) => (
                PermissionVerdict::Error("invalid".to_string()),
                format!("Policy: INVALID ({})", err),
            ),
        },
    }
}

/// Read and parse the policy file. Prints one status line and returns the verdict.
fn check_policy(path: &Path) -> PermissionVerdict {
    let (verdict, msg) = policy_status(path);
    if !msg.is_empty() {
        println!("{msg}");
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn policy_missing_file_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.yaml");
        let (verdict, msg) = policy_status(&path);
        assert!(matches!(verdict, PermissionVerdict::Error(_)));
        assert!(msg.contains("Policy: MISSING"), "{msg}");
    }

    #[test]
    fn policy_valid_file_reports_ok() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "rules:\n  - user: \"*\"\n    org: acme\n    repo: \"*\"\n    operations: [push]\n    effect: allow"
        )
        .unwrap();
        let (verdict, _msg) = policy_status(f.path());
        assert_eq!(verdict, PermissionVerdict::Ok);
    }

    #[test]
    fn policy_invalid_yaml_reports_invalid() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "not: valid: policy: content: !!").unwrap();
        let (verdict, msg) = policy_status(f.path());
        assert!(matches!(verdict, PermissionVerdict::Error(_)));
        assert!(msg.contains("INVALID"), "{msg}");
    }

    #[test]
    fn classify_permissions_tiers_by_path_kind() {
        const GHBRK_UID: u32 = 4242;
        const OTHER_UID: u32 = 1000;
        const FILE_TYPE_BITS: u32 = 0o100000;

        // --- File path kind ---------------------------------------------------
        assert_eq!(
            classify_permissions(PathKind::File, GHBRK_UID, GHBRK_UID, 0o600, 0o600),
            PermissionVerdict::Ok,
            "correct owner + exact 0600 file is OK"
        );

        assert_eq!(
            classify_permissions(
                PathKind::File,
                GHBRK_UID,
                GHBRK_UID,
                0o600,
                FILE_TYPE_BITS | 0o600
            ),
            PermissionVerdict::Ok,
            "raw st_mode with file-type bits should be masked to permission bits"
        );

        match classify_permissions(PathKind::File, GHBRK_UID, GHBRK_UID, 0o600, 0o640) {
            PermissionVerdict::Warning(detail) => assert!(detail.contains("640")),
            other => panic!("file group-read 0640 should WARN, got {other:?}"),
        }

        match classify_permissions(PathKind::File, GHBRK_UID, GHBRK_UID, 0o600, 0o644) {
            PermissionVerdict::Warning(detail) => assert!(detail.contains("644")),
            other => panic!("file other-read 0644 should WARN, got {other:?}"),
        }

        match classify_permissions(PathKind::File, GHBRK_UID, GHBRK_UID, 0o600, 0o620) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("620")),
            other => panic!("file group-write 0620 should ERROR, got {other:?}"),
        }

        match classify_permissions(PathKind::File, GHBRK_UID, GHBRK_UID, 0o600, 0o666) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("666")),
            other => panic!("file group/other-write 0666 should ERROR, got {other:?}"),
        }

        match classify_permissions(PathKind::File, GHBRK_UID, OTHER_UID, 0o600, 0o600) {
            PermissionVerdict::Error(detail) => {
                assert!(detail.contains("1000"), "detail should name observed owner");
                assert!(detail.contains("4242"), "detail should name expected owner");
            }
            other => panic!("file wrong owner should ERROR even at 0600, got {other:?}"),
        }

        assert!(
            matches!(
                classify_permissions(PathKind::File, GHBRK_UID, OTHER_UID, 0o600, 0o666),
                PermissionVerdict::Error(_)
            ),
            "wrong owner dominates over write-path mode"
        );

        // --- Directory path kind ----------------------------------------------
        assert_eq!(
            classify_permissions(PathKind::Directory, 0, 0, 0o755, 0o755),
            PermissionVerdict::Ok,
            "root-owned 0755 directory is OK (group/other read+exec is intended)"
        );

        assert_eq!(
            classify_permissions(PathKind::Directory, 0, 0, 0o700, 0o700),
            PermissionVerdict::Ok,
            "owner-only 0700 directory is OK"
        );

        // 0o740: group read but no group/other execute -> read-path widening only.
        match classify_permissions(PathKind::Directory, 0, 0, 0o700, 0o740) {
            PermissionVerdict::Warning(detail) => assert!(detail.contains("740")),
            other => panic!("dir group-read-only 0740 should WARN, got {other:?}"),
        }

        // 0o710: group execute (traversal) is write-path even with no write bit.
        match classify_permissions(PathKind::Directory, 0, 0, 0o700, 0o710) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("710")),
            other => panic!("dir group-exec 0710 should ERROR (traversal), got {other:?}"),
        }

        match classify_permissions(PathKind::Directory, 0, 0, 0o700, 0o720) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("720")),
            other => panic!("dir group-write 0720 should ERROR, got {other:?}"),
        }

        match classify_permissions(PathKind::Directory, 0, 0, 0o755, 0o757) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("757")),
            other => panic!("dir world-write 0757 should ERROR, got {other:?}"),
        }

        match classify_permissions(PathKind::Directory, 0, OTHER_UID, 0o755, 0o755) {
            PermissionVerdict::Error(detail) => {
                assert!(detail.contains("1000"), "detail should name observed owner");
                assert!(detail.contains('0'), "detail should name expected owner");
            }
            other => panic!("dir wrong owner should ERROR, got {other:?}"),
        }

        // --- Socket path kind -------------------------------------------------
        assert_eq!(
            classify_permissions(PathKind::Socket, GHBRK_UID, GHBRK_UID, 0o660, 0o660),
            PermissionVerdict::Ok,
            "ghbrk-owned 0660 socket is OK (group connect is intended)"
        );

        assert_eq!(
            classify_permissions(PathKind::Socket, GHBRK_UID, GHBRK_UID, 0o660, 0o600),
            PermissionVerdict::Ok,
            "tighter 0600 socket is OK"
        );

        // 0o666: other read+write -> non-group connect -> ERROR.
        match classify_permissions(PathKind::Socket, GHBRK_UID, GHBRK_UID, 0o660, 0o666) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("666")),
            other => panic!("socket world-connectable 0666 should ERROR, got {other:?}"),
        }

        // 0o664: other-read only is still a non-group exposure -> ERROR.
        match classify_permissions(PathKind::Socket, GHBRK_UID, GHBRK_UID, 0o660, 0o664) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("664")),
            other => panic!("socket other-read 0664 should ERROR, got {other:?}"),
        }

        match classify_permissions(PathKind::Socket, GHBRK_UID, OTHER_UID, 0o660, 0o660) {
            PermissionVerdict::Error(detail) => {
                assert!(detail.contains("1000"), "detail should name observed owner");
                assert!(detail.contains("4242"), "detail should name expected owner");
            }
            other => panic!("socket wrong owner should ERROR, got {other:?}"),
        }
    }

    #[test]
    fn config_dir_from_policy_path_uses_parent() {
        assert_eq!(
            config_dir_from_policy_path(Path::new("/etc/ghbrk/policy.yaml")),
            PathBuf::from("/etc/ghbrk")
        );
    }

    #[test]
    fn config_dir_from_bare_policy_path_falls_back_to_default() {
        assert_eq!(
            config_dir_from_policy_path(Path::new("policy.yaml")),
            PathBuf::from(DEFAULT_CONFIG_DIR)
        );
    }

    #[test]
    fn config_dir_missing_path_reports_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent");
        match check_config_dir_permissions(&missing) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("cannot stat")),
            other => panic!("missing config dir should ERROR, got {other:?}"),
        }
    }

    #[test]
    fn config_dir_owned_by_non_root_reports_error() {
        // A tempdir is owned by the (non-root) test runner, so the root-owner
        // expectation is violated: a wrong owner is a write-path ERROR.
        let dir = tempfile::tempdir().unwrap();
        match check_config_dir_permissions(dir.path()) {
            PermissionVerdict::Error(detail) => assert!(detail.contains("owner uid")),
            PermissionVerdict::Ok => { /* test happens to run as root */ }
            other => panic!("non-root-owned config dir should ERROR, got {other:?}"),
        }
    }

    #[test]
    fn socket_missing_path_reports_error_or_owner_warning() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.sock");
        match check_socket_permissions(&missing) {
            // When the `ghbrk` user exists, the stat failure is the ERROR; on a
            // host without the service account the owner lookup warns first.
            PermissionVerdict::Error(detail) => assert!(detail.contains("cannot stat")),
            PermissionVerdict::Warning(detail) => {
                assert!(detail.contains(SOCKET_OWNER_USER));
            }
            other => panic!("missing socket should ERROR or WARN, got {other:?}"),
        }
    }

    #[test]
    fn aggregate_exit_is_success_when_all_ok() {
        let verdicts = [
            PermissionVerdict::Ok,
            PermissionVerdict::Ok,
            PermissionVerdict::Ok,
        ];
        assert_eq!(
            aggregate_exit(&verdicts),
            ExitCode::SUCCESS,
            "all-OK aggregate must exit success"
        );
    }

    #[test]
    fn aggregate_exit_is_success_when_only_warnings() {
        let verdicts = [
            PermissionVerdict::Ok,
            PermissionVerdict::Warning("read-path widening".into()),
            PermissionVerdict::Warning("another".into()),
        ];
        assert_eq!(
            aggregate_exit(&verdicts),
            ExitCode::SUCCESS,
            "warnings without errors must not fail the exit status"
        );
    }

    #[test]
    fn aggregate_exit_fails_when_any_error_present() {
        let verdicts = [
            PermissionVerdict::Ok,
            PermissionVerdict::Warning("read-path".into()),
            PermissionVerdict::Error("write-path".into()),
        ];
        assert_eq!(
            aggregate_exit(&verdicts),
            ExitCode::FAILURE,
            "a single error among warnings must fail the exit status"
        );
    }

    #[test]
    fn aggregate_exit_of_empty_is_success() {
        assert_eq!(aggregate_exit(&[]), ExitCode::SUCCESS);
    }

    #[test]
    fn verdict_from_success_maps_true_to_ok() {
        assert_eq!(
            verdict_from_success(true, "unused detail"),
            PermissionVerdict::Ok
        );
    }

    #[test]
    fn verdict_from_success_maps_false_to_error_with_detail() {
        match verdict_from_success(false, "daemon unreachable") {
            PermissionVerdict::Error(detail) => assert_eq!(detail, "daemon unreachable"),
            other => panic!("false should map to Error, got {other:?}"),
        }
    }

    #[test]
    fn policy_permission_denied_is_silent() {
        use std::os::unix::fs::PermissionsExt;

        // Skip this test when running as root (root can read any file).
        if nix::unistd::getuid().is_root() {
            return;
        }

        let f = NamedTempFile::new().unwrap();
        std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let (verdict, msg) = policy_status(f.path());
        assert!(
            matches!(verdict, PermissionVerdict::Ok),
            "PermissionDenied should be Ok (silent), got: {verdict:?}"
        );
        assert!(
            msg.is_empty(),
            "message must be empty for PermissionDenied, got: {msg}"
        );
    }

    #[tokio::test]
    async fn daemon_unreachable_reports_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let (daemon_ok, checks) = check_daemon_and_creds(&socket, None).await;
        assert!(!daemon_ok);
        assert!(!checks.credentials_ok);
        assert!(checks.credential_paths.is_empty());
        assert_eq!(
            checks.shared_filesystem,
            PermissionVerdict::Ok,
            "a check that never ran must not fail the exit status on its own"
        );
    }

    #[test]
    fn caller_tmp_identity_reports_own_tmp_stat() {
        use std::os::unix::fs::MetadataExt;

        let meta = std::fs::metadata("/tmp").unwrap();
        let identity = caller_tmp_identity().expect("stat of /tmp should succeed in tests");
        assert_eq!(identity.dev, meta.dev());
        assert_eq!(identity.ino, meta.ino());
    }

    #[test]
    fn split_out_shared_filesystem_line_extracts_matching_line() {
        let output = "SSH key: OK\nToken: OK\nShared filesystem: OK\nGitHub API: OK (user: x)\n";
        let (line, remaining) = split_out_shared_filesystem_line(output);
        assert_eq!(line, Some("Shared filesystem: OK".to_string()));
        assert_eq!(
            remaining,
            "SSH key: OK\nToken: OK\nGitHub API: OK (user: x)\n"
        );
    }

    /// The credential status lines a healthy credential set produces.
    const PASSING_CREDENTIAL_LINES: &str = "SSH key: OK\nToken: OK\nGitHub API: OK (user: x)\n";

    /// Exit code the broker returns when at least one check it ran failed.
    const BROKER_CHECK_FAILED: i32 = 1;

    #[test]
    fn credentials_pass_when_every_line_reports_ok_despite_a_failing_exit_code() {
        assert!(
            credentials_passed(BROKER_CHECK_FAILED, PASSING_CREDENTIAL_LINES, &[]),
            "a non-zero exit caused by another check must not fail the credentials"
        );
    }

    #[test]
    fn credentials_fail_when_a_credential_line_reports_a_failure() {
        let output = "SSH key: OK\nToken: MISSING\nGitHub API: SKIPPED (no token available)\n";
        assert!(!credentials_passed(BROKER_CHECK_FAILED, output, &[]));
    }

    #[test]
    fn credentials_treat_a_skipped_check_as_passing() {
        let output = "SSH key: OK\nToken: OK\nGitHub API: SKIPPED (no token available)\n";
        assert!(credentials_passed(BROKER_CHECK_FAILED, output, &[]));
    }

    #[test]
    fn credentials_fail_for_an_unrecognised_line_when_the_exit_code_failed() {
        let output = "ghbrk check: invalid user name \"../etc\"\n";
        assert!(
            !credentials_passed(BROKER_CHECK_FAILED, output, &[]),
            "a line this client cannot classify must fail closed"
        );
    }

    #[test]
    fn credentials_pass_for_an_unrecognised_line_when_the_exit_code_succeeded() {
        let output = "Some check a newer daemon added: PENDING\n";
        assert!(
            credentials_passed(0, output, &[]),
            "a zero exit means the broker passed every check it ran"
        );
    }

    #[test]
    fn credentials_fail_when_a_credential_path_audit_reports_an_error() {
        let verdicts = [PermissionVerdict::Error(
            "mode 0620 is group-writable".into(),
        )];
        assert!(!credentials_passed(0, PASSING_CREDENTIAL_LINES, &verdicts));
    }

    #[test]
    fn credentials_pass_when_a_credential_path_audit_only_warns() {
        let verdicts = [PermissionVerdict::Warning(
            "mode 0640 is group-readable".into(),
        )];
        assert!(credentials_passed(0, PASSING_CREDENTIAL_LINES, &verdicts));
    }

    #[test]
    fn credentials_pass_when_the_broker_reported_no_credential_lines() {
        assert!(
            credentials_passed(BROKER_CHECK_FAILED, "", &[]),
            "no credential line reported a failure, so no credential failed"
        );
    }

    #[test]
    fn shared_filesystem_verdict_is_ok_for_a_passing_line() {
        assert_eq!(
            shared_filesystem_verdict("Shared filesystem: OK"),
            PermissionVerdict::Ok
        );
    }

    #[test]
    fn shared_filesystem_verdict_is_an_error_naming_the_failing_line() {
        let line = "Shared filesystem: ERROR (caller /tmp is dev 8:1, broker /tmp is dev 0:34)";
        match shared_filesystem_verdict(line) {
            PermissionVerdict::Error(detail) => assert_eq!(detail, line),
            other => panic!("a failing shared-filesystem line should ERROR, got {other:?}"),
        }
    }

    #[test]
    fn split_out_shared_filesystem_line_reports_none_when_absent() {
        let output = "SSH key: OK\nToken: OK\n";
        let (line, remaining) = split_out_shared_filesystem_line(output);
        assert_eq!(line, None);
        assert_eq!(remaining, output);
    }
}
