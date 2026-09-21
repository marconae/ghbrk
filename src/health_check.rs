//! Credential health checks shared by `ghbrk check` and the broker.
//!
//! These checks run inside the broker process, as the `ghbrk` user. This
//! lets them read credential files that only `ghbrk` can read. Each check
//! writes its output to a caller-supplied writer, not to stdout, so the
//! broker can stream the output back to the client.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::credentials::{PERMISSION_MASK, REQUIRED_MODE};
use crate::protocol::{CredentialAudit, PathAudit, TmpIdentity};

/// Label for the per-user credential directory entry in a [`CredentialAudit`].
const CREDENTIAL_DIR_LABEL: &str = "Credential dir";

/// Label for the SSH key entry in a [`CredentialAudit`].
const SSH_KEY_LABEL: &str = "SSH key";

/// Label for the token entry in a [`CredentialAudit`].
const TOKEN_LABEL: &str = "Token";

/// Outcome of pinging the GitHub user endpoint.
pub enum GithubResult {
    Ok(String),
    InvalidToken,
    Unreachable,
}

/// Runs all credential health checks for `user` under `creds_root`. Writes
/// one status line per check to `out`.
///
/// When `caller_tmp` carries the caller's own `/tmp` identity, this function
/// also runs the shared-filesystem check against the broker's own `/tmp`.
/// When `caller_tmp` is absent, the client predates that check: this
/// function then writes no `Shared filesystem:` line, and the check does
/// not affect the result. Returns `true` only when every check that runs
/// passes.
///
/// The shared-filesystem check runs first and does not depend on the
/// credential checks. It compares two integers on the local host, so it
/// must not wait for the GitHub round trip. A daemon that supports this
/// check must report it on every path. A client reads a missing `Shared
/// filesystem:` line as version skew, so skipping the line for an unrelated
/// credential fault would point to the wrong cause.
pub fn run_checks(inputs: CheckInputs<'_>, out: &mut impl Write) -> bool {
    let mut all_ok = true;
    if let Some(caller_tmp) = inputs.caller_tmp {
        all_ok &= check_shared_filesystem(Path::new(BROKER_TMP_PATH), caller_tmp, out);
    }

    let paths = match crate::credentials::credential_paths_in(inputs.creds_root, inputs.user) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "ghbrk check: {e}");
            return false;
        }
    };

    all_ok &= check_file("SSH key", &paths.ssh_key, out);
    all_ok &= check_file("Token", &paths.token, out);
    check_github_api(&paths.token, &mut all_ok, out);
    all_ok
}

/// Inputs for [`run_checks`]: the credential root and user to check, and the
/// caller's own `/tmp` identity when the client sent one. This struct groups
/// the inputs into one value, so `run_checks` and `handle_check_request`
/// (which builds this value) stay within the function-argument-count
/// guardrail as more checks arrive.
pub struct CheckInputs<'a> {
    pub creds_root: &'a Path,
    pub user: &'a str,
    pub caller_tmp: Option<TmpIdentity>,
}

/// Path of the broker's own `/tmp`, checked by the shared-filesystem check.
const BROKER_TMP_PATH: &str = "/tmp";

/// Prefix of the shared-filesystem check's status line. `src/cmd/doctor.rs`
/// detects the line by this same prefix, and also emits two more lines with
/// it (`SKIPPED`, `UNSUPPORTED`). Both sides must spell the prefix the same
/// way, or the wire contract between them breaks.
pub const SHARED_FILESYSTEM_LABEL: &str = "Shared filesystem:";

/// Path of the installed systemd unit file. The remediation message for a
/// mismatch names this file: removing `PrivateTmp=` from it and restarting
/// the service restores a shared `/tmp` for the spawned child.
const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/ghbrk.service";

/// Stats `tmp_path` (the broker's own `/tmp` in production) and compares it
/// against `caller_tmp`, the identity the caller stat'd in its own
/// namespace. Writes one [`SHARED_FILESYSTEM_LABEL`] line and returns
/// whether the two identities match.
///
/// This function never resolves or opens a caller-named path. It compares
/// two integers against a value that it derives from a fixed, root-owned
/// path, so it has no symlink, FIFO, or time-of-check/time-of-use attack
/// surface. `tmp_path` is a parameter, not a fixed read of
/// [`BROKER_TMP_PATH`], so a test can reach the stat-failure branch.
fn check_shared_filesystem(tmp_path: &Path, caller_tmp: TmpIdentity, out: &mut impl Write) -> bool {
    let broker_tmp = match fs::metadata(tmp_path) {
        Ok(meta) => TmpIdentity {
            dev: meta.dev(),
            ino: meta.ino(),
        },
        Err(err) => {
            let _ = writeln!(
                out,
                "{SHARED_FILESYSTEM_LABEL} ERROR (cannot stat /tmp: {err})"
            );
            return false;
        }
    };

    if broker_tmp.dev == caller_tmp.dev && broker_tmp.ino == caller_tmp.ino {
        let _ = writeln!(out, "{SHARED_FILESYSTEM_LABEL} OK");
        return true;
    }

    let mount_root_clause = match own_tmp_mount_root() {
        Some(root) => format!("; broker /tmp mount root is {root}"),
        None => String::new(),
    };
    let _ = writeln!(
        out,
        "{SHARED_FILESYSTEM_LABEL} ERROR (caller /tmp is dev {}:{}, broker /tmp is dev {}:{}; \
         caused by PrivateTmp= in {SYSTEMD_UNIT_PATH}{mount_root_clause}; \
         remove that directive from {SYSTEMD_UNIT_PATH} and restart the service)",
        caller_tmp.dev, caller_tmp.ino, broker_tmp.dev, broker_tmp.ino
    );
    false
}

/// Reads the broker's own `/proc/self/mountinfo` and extracts the `/tmp`
/// mount root via [`tmp_mount_root`], or `None` when the file cannot be read.
fn own_tmp_mount_root() -> Option<String> {
    let content = fs::read_to_string("/proc/self/mountinfo").ok()?;
    tmp_mount_root(&content)
}

/// Stats the caller's credential directory and each credential file, and
/// records the observed owner uid and mode of every path. The broker runs
/// this as the privileged service account, because the caller cannot stat
/// these paths directly. The broker then sends the result over the socket,
/// so `doctor` can run the tiered permission classifier against it.
///
/// Returns an audit with no entries when the user name does not resolve to
/// a credential path. The text-based [`run_checks`] reports that error on
/// its own.
pub fn audit_credential_paths(creds_root: &Path, user: &str) -> CredentialAudit {
    let paths = match crate::credentials::credential_paths_in(creds_root, user) {
        Ok(p) => p,
        Err(_) => return CredentialAudit::default(),
    };

    let mut entries = Vec::with_capacity(3);
    if let Some(dir) = paths.ssh_key.parent() {
        entries.push(audit_path(CREDENTIAL_DIR_LABEL, dir));
    }
    entries.push(audit_path(SSH_KEY_LABEL, &paths.ssh_key));
    entries.push(audit_path(TOKEN_LABEL, &paths.token));

    CredentialAudit { entries }
}

/// Stats one path and records it as a [`PathAudit`]. When the path does not
/// exist, or the stat fails, this function records the path as absent with
/// a zeroed owner and mode. The classifier on the client treats an absent
/// path differently from a widened permission.
fn audit_path(label: &str, path: &Path) -> PathAudit {
    match fs::metadata(path) {
        Ok(meta) => PathAudit {
            label: label.to_string(),
            path: path.to_path_buf(),
            present: true,
            observed_owner_uid: meta.uid(),
            observed_mode: meta.mode() & PERMISSION_MASK,
        },
        Err(_) => PathAudit {
            label: label.to_string(),
            path: path.to_path_buf(),
            present: false,
            observed_owner_uid: 0,
            observed_mode: 0,
        },
    }
}

fn check_file(label: &str, path: &Path, out: &mut impl Write) -> bool {
    match fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(out, "{label}: MISSING");
            false
        }
        Err(e) => {
            let _ = writeln!(out, "{label}: ERROR ({e})");
            false
        }
        Ok(meta) => {
            let mode = meta.permissions().mode() & PERMISSION_MASK;
            if mode == REQUIRED_MODE {
                let _ = writeln!(out, "{label}: OK");
                true
            } else {
                let _ = writeln!(out, "{label}: BAD PERMISSIONS ({mode:#05o})");
                false
            }
        }
    }
}

/// Index of the mount root field in a `/proc/self/mountinfo` line.
const MOUNTINFO_ROOT_FIELD: usize = 3;

/// Index of the mount point field in a `/proc/self/mountinfo` line.
const MOUNTINFO_MOUNT_POINT_FIELD: usize = 4;

/// Extracts the mount root of the last `/proc/self/mountinfo` entry whose
/// mount point is `/tmp`. Returns `None` when that root is `/`, which marks
/// an ordinary mount rather than a bind mount that substitutes a private
/// directory for `/tmp`.
///
/// A `PrivateTmp=true` systemd unit bind-mounts a private directory onto
/// `/tmp` inside its own mount namespace. The mount point stays `/tmp`,
/// while the mount root records the substitution as a path that starts
/// with `/systemd-private-`. This function reads only the broker's own
/// mountinfo content. It takes no input from the caller.
fn tmp_mount_root(mountinfo: &str) -> Option<String> {
    let mut root = None;
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        if fields.len() > MOUNTINFO_MOUNT_POINT_FIELD
            && fields[MOUNTINFO_MOUNT_POINT_FIELD] == "/tmp"
        {
            root = Some(fields[MOUNTINFO_ROOT_FIELD].to_string());
        }
    }
    root.filter(|root| root != "/")
}

fn check_github_api(token_path: &Path, ok: &mut bool, out: &mut impl Write) {
    let token = match read_token_if_available(token_path) {
        Some(t) => t,
        None => {
            let _ = writeln!(out, "GitHub API: SKIPPED (no token available)");
            return;
        }
    };

    match ping_github(&token) {
        GithubResult::Ok(login) => {
            let _ = writeln!(out, "GitHub API: OK (user: {login})");
        }
        GithubResult::InvalidToken => {
            let _ = writeln!(out, "GitHub API: INVALID TOKEN");
            *ok = false;
        }
        GithubResult::Unreachable => {
            let _ = writeln!(out, "GitHub API: UNREACHABLE");
            *ok = false;
        }
    }
}

fn read_token_if_available(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(raw) => Some(raw.trim_end_matches(['\n', '\r']).to_string()),
        Err(_) => None,
    }
}

/// Wall-clock budget, in seconds, for the `gh api user` probe.
/// `Command::output` has no timeout of its own, so this call runs inside the
/// coreutils `timeout` binary.
// ponytail: fixed timeout, bump if health checks against slow networks start failing
const GH_PROBE_TIMEOUT_SECS: &str = "10";

/// Substring that `gh` writes to stderr when GitHub rejects the token.
/// [`ping_github`] classifies any other failure as unreachable, not as a bad
/// credential.
const UNAUTHORIZED_MARKER: &str = "HTTP 401";

/// Validates `token` by running `gh api user` as a subprocess, the same way
/// every other remote operation in the broker reaches GitHub. `gh`'s `-q`
/// query engine extracts the login, so this function does not parse JSON.
fn ping_github(token: &str) -> GithubResult {
    let output = std::process::Command::new("timeout")
        .args([GH_PROBE_TIMEOUT_SECS, "gh", "api", "user", "-q", ".login"])
        .envs(crate::credentials::gh_env(token))
        .output();

    let output = match output {
        Ok(output) => output,
        Err(_) => return GithubResult::Unreachable,
    };

    if output.status.success() {
        return GithubResult::Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    // `gh` reports a rejected token as `gh: Bad credentials (HTTP 401)`.
    if String::from_utf8_lossy(&output.stderr).contains(UNAUTHORIZED_MARKER) {
        return GithubResult::InvalidToken;
    }
    GithubResult::Unreachable
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_mode(path: &Path, contents: &str, mode: u32) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn inputs<'a>(
        creds_root: &'a Path,
        user: &'a str,
        caller_tmp: Option<TmpIdentity>,
    ) -> CheckInputs<'a> {
        CheckInputs {
            creds_root,
            user,
            caller_tmp,
        }
    }

    /// Serializes tests that mutate the process-wide `PATH`. Without this
    /// lock, parallel `cargo test` threads would race on that shared state.
    static PATH_MUTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores the `PATH` value from before [`install_stub_gh`] and
    /// releases the serialization lock. Keep this guard alive for as long
    /// as a child process must resolve the stub.
    struct PathGuard {
        _exclusive: std::sync::MutexGuard<'static, ()>,
        original: Option<String>,
    }

    impl Drop for PathGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    /// Writes a stub `gh` whose body is `body` into `dir` and puts `dir` first
    /// on `PATH`, so `ping_github` resolves the stub instead of a real `gh`.
    fn install_stub_gh(dir: &Path, body: &str) -> PathGuard {
        let exclusive = PATH_MUTATION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        write_mode(&dir.join("gh"), &format!("#!/bin/sh\n{body}\n"), 0o755);
        let original = std::env::var("PATH").ok();
        let prev = original.clone().unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", dir.display(), prev));
        PathGuard {
            _exclusive: exclusive,
            original,
        }
    }

    /// Runs `check_github_api` against a token file in `dir` and returns the
    /// emitted status line plus whether the check left the overall result ok.
    fn github_api_line(dir: &Path) -> (String, bool) {
        let token = dir.join("token");
        fs::write(&token, "tok").unwrap();
        let mut ok = true;
        let mut out = Vec::new();
        check_github_api(&token, &mut ok, &mut out);
        (String::from_utf8(out).unwrap(), ok)
    }

    #[test]
    fn ping_github_reports_ok_with_login_from_stdout() {
        let dir = TempDir::new().unwrap();
        let _path = install_stub_gh(dir.path(), "echo octocat");

        match ping_github("tok") {
            GithubResult::Ok(login) => assert_eq!(login, "octocat"),
            _ => panic!("a successful `gh api user` must classify as Ok"),
        }

        let (line, ok) = github_api_line(dir.path());
        assert!(ok, "{line}");
        assert!(line.contains("GitHub API: OK (user: octocat)"), "{line}");
    }

    #[test]
    fn ping_github_reports_invalid_token_on_http_401() {
        let dir = TempDir::new().unwrap();
        let _path = install_stub_gh(
            dir.path(),
            "echo 'gh: Bad credentials (HTTP 401)' >&2\nexit 1",
        );

        assert!(
            matches!(ping_github("tok"), GithubResult::InvalidToken),
            "an HTTP 401 from `gh api user` must classify as InvalidToken"
        );

        let (line, ok) = github_api_line(dir.path());
        assert!(!ok, "{line}");
        assert!(line.contains("GitHub API: INVALID TOKEN"), "{line}");
    }

    #[test]
    fn ping_github_reports_unreachable_for_unrelated_failure() {
        let dir = TempDir::new().unwrap();
        let _path = install_stub_gh(
            dir.path(),
            "echo 'dial tcp: lookup api.github.com: no such host' >&2\nexit 1",
        );

        assert!(
            matches!(ping_github("tok"), GithubResult::Unreachable),
            "a failure that is not an HTTP 401 must classify as Unreachable"
        );

        let (line, ok) = github_api_line(dir.path());
        assert!(!ok, "{line}");
        assert!(line.contains("GitHub API: UNREACHABLE"), "{line}");
    }

    #[test]
    fn run_checks_reports_missing_files() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        let ok = run_checks(inputs(dir.path(), "alice", None), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(!ok);
        assert!(s.contains("SSH key: MISSING"), "{s}");
        assert!(s.contains("Token: MISSING"), "{s}");
    }

    #[test]
    fn run_checks_reports_ok_for_well_formed_credentials() {
        let dir = TempDir::new().unwrap();
        write_mode(&dir.path().join("alice/id_rsa"), "KEY", 0o600);
        write_mode(&dir.path().join("alice/token"), "tok", 0o600);
        let bin = TempDir::new().unwrap();
        let _path = install_stub_gh(bin.path(), "echo 'connection refused' >&2\nexit 1");
        let mut out = Vec::new();
        let ok = run_checks(inputs(dir.path(), "alice", None), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("SSH key: OK"), "{s}");
        assert!(s.contains("Token: OK"), "{s}");
        // GitHub API is unreachable here, so overall not ok.
        assert!(!ok);
        assert!(s.contains("GitHub API: UNREACHABLE"), "{s}");
    }

    #[test]
    fn run_checks_rejects_invalid_user() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        let ok = run_checks(inputs(dir.path(), "../etc", None), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(!ok);
        assert!(s.contains("ghbrk check:"), "{s}");
    }

    #[test]
    fn run_checks_reports_shared_filesystem_when_credential_path_is_invalid() {
        let dir = TempDir::new().unwrap();
        let meta = fs::metadata("/tmp").unwrap();
        let caller_tmp = crate::protocol::TmpIdentity {
            dev: meta.dev(),
            ino: meta.ino(),
        };
        let mut out = Vec::new();
        let ok = run_checks(inputs(dir.path(), "../etc", Some(caller_tmp)), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(!ok, "an unresolvable credential path must fail the check");
        assert!(s.contains("ghbrk check:"), "{s}");
        assert!(
            s.contains("Shared filesystem:"),
            "a supported daemon must report the shared-filesystem check even \
             when the credential path cannot be resolved: {s}"
        );
    }

    #[test]
    fn run_checks_omits_shared_filesystem_line_when_identity_absent() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        run_checks(inputs(dir.path(), "alice", None), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(!s.contains("Shared filesystem:"), "{s}");
    }

    #[test]
    fn run_checks_reports_ok_for_matching_tmp_identity() {
        let dir = TempDir::new().unwrap();
        let meta = fs::metadata("/tmp").unwrap();
        let caller_tmp = crate::protocol::TmpIdentity {
            dev: meta.dev(),
            ino: meta.ino(),
        };
        let mut out = Vec::new();
        run_checks(inputs(dir.path(), "alice", Some(caller_tmp)), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Shared filesystem: OK"), "{s}");
    }

    #[test]
    fn run_checks_reports_error_for_mismatched_tmp_identity() {
        let dir = TempDir::new().unwrap();
        let meta = fs::metadata("/tmp").unwrap();
        let caller_tmp = crate::protocol::TmpIdentity {
            dev: meta.dev(),
            ino: meta.ino().wrapping_add(1),
        };
        let mut out = Vec::new();
        let ok = run_checks(inputs(dir.path(), "alice", Some(caller_tmp)), &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(!ok);
        assert!(s.contains("Shared filesystem: ERROR"), "{s}");
        assert!(s.contains("PrivateTmp="), "{s}");
        assert!(
            s.contains("/etc/systemd/system/ghbrk.service"),
            "expected remediation naming the unit file: {s}"
        );
        assert!(!s.contains("reinstall"), "{s}");
    }

    #[test]
    fn check_shared_filesystem_reports_error_when_stat_fails() {
        let dir = TempDir::new().unwrap();
        let missing_tmp = dir.path().join("does-not-exist");
        let caller_tmp = TmpIdentity { dev: 0, ino: 0 };
        let mut out = Vec::new();
        let ok = check_shared_filesystem(&missing_tmp, caller_tmp, &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(
            !ok,
            "a stat failure on the broker's own /tmp must fail closed"
        );
        assert!(
            s.starts_with(SHARED_FILESYSTEM_LABEL),
            "expected the shared-filesystem label: {s}"
        );
        assert!(s.contains("ERROR"), "{s}");
        assert!(
            s.contains("cannot stat"),
            "expected the stat failure to be named: {s}"
        );
    }

    #[test]
    fn audit_reports_owner_and_mode_for_dir_and_files() {
        let dir = TempDir::new().unwrap();
        write_mode(&dir.path().join("alice/id_rsa"), "KEY", 0o600);
        write_mode(&dir.path().join("alice/token"), "tok", 0o640);
        fs::set_permissions(dir.path().join("alice"), fs::Permissions::from_mode(0o700)).unwrap();

        let audit = audit_credential_paths(dir.path(), "alice");
        let by_label = |label: &str| {
            audit
                .entries
                .iter()
                .find(|e| e.label == label)
                .unwrap_or_else(|| panic!("missing entry for {label}"))
        };

        let me = nix::unistd::geteuid().as_raw();

        let dir_entry = by_label("Credential dir");
        assert!(dir_entry.present);
        assert_eq!(dir_entry.observed_mode, 0o700);
        assert_eq!(dir_entry.observed_owner_uid, me);
        assert!(dir_entry.path.ends_with("alice"));

        let key_entry = by_label("SSH key");
        assert!(key_entry.present);
        assert_eq!(key_entry.observed_mode, 0o600);
        assert_eq!(key_entry.observed_owner_uid, me);

        let token_entry = by_label("Token");
        assert!(token_entry.present);
        assert_eq!(token_entry.observed_mode, 0o640);
    }

    #[test]
    fn audit_marks_missing_paths_absent_with_zeroed_owner_mode() {
        let dir = TempDir::new().unwrap();
        let audit = audit_credential_paths(dir.path(), "ghost");
        assert_eq!(audit.entries.len(), 3);
        for entry in &audit.entries {
            assert!(!entry.present, "{} should be absent", entry.label);
            assert_eq!(entry.observed_owner_uid, 0);
            assert_eq!(entry.observed_mode, 0);
        }
    }

    #[test]
    fn audit_invalid_user_yields_no_entries() {
        let dir = TempDir::new().unwrap();
        let audit = audit_credential_paths(dir.path(), "../etc");
        assert!(audit.entries.is_empty());
    }

    #[test]
    fn tmp_mount_root_reports_private_namespace_root() {
        let mountinfo = "25 30 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
             113 25 0:34 /systemd-private-9302792eb91a42bbbeef83f282d9bd85-ghbrk.service-ApAiqY/tmp /tmp rw,nosuid,nodev shared:60 - tmpfs tmpfs rw\n";
        assert_eq!(
            tmp_mount_root(mountinfo),
            Some(
                "/systemd-private-9302792eb91a42bbbeef83f282d9bd85-ghbrk.service-ApAiqY/tmp"
                    .to_string()
            )
        );
    }

    #[test]
    fn tmp_mount_root_reports_none_for_plain_root() {
        let mountinfo = "25 30 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
             40 25 8:1 / /tmp rw,relatime shared:1 - ext4 /dev/sda1 rw\n";
        assert_eq!(tmp_mount_root(mountinfo), None);
    }

    #[test]
    fn tmp_mount_root_reports_none_when_no_tmp_entry() {
        let mountinfo = "25 30 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n";
        assert_eq!(tmp_mount_root(mountinfo), None);
    }

    #[test]
    fn tmp_mount_root_uses_last_matching_entry() {
        let mountinfo = "25 30 8:1 /first /tmp rw shared:1 - ext4 /dev/sda1 rw\n\
             40 25 0:34 /systemd-private-abc/tmp /tmp rw shared:60 - tmpfs tmpfs rw\n";
        assert_eq!(
            tmp_mount_root(mountinfo),
            Some("/systemd-private-abc/tmp".to_string())
        );
    }
}
