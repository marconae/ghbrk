use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nix::unistd::{chown, Group};
use thiserror::Error;

/// Root directory under which per-user credentials live.
const CREDENTIALS_ROOT: &str = "/etc/ghbrk/credentials";

/// File name for the SSH private key.
const SSH_KEY_FILE: &str = "id_rsa";

/// File name for the GitHub token.
const TOKEN_FILE: &str = "token";

/// Required permission bits on credential files: 0o600 (owner read+write, no
/// group, no other).
const REQUIRED_MODE: u32 = 0o600;

/// Mask isolating the permission bits of a unix mode.
const PERMISSION_MASK: u32 = 0o777;

/// Group whose members are permitted to connect to the per-operation
/// ssh-agent socket. Mirrors `broker::CLIENT_GROUP_NAME`; the daemon runs with
/// this as a supplementary group and privilege-dropped git children inherit it.
const CLIENT_GROUP_NAME: &str = "ghbrk-clients";

/// Lifetime (seconds) of a key loaded into the per-operation ssh-agent via
/// `ssh-add -t`. The agent is killed when the operation completes
/// (`SshAgentHandle::drop`), but the TTL bounds key residency as defense in
/// depth in case cleanup is skipped (e.g. the daemon is SIGKILLed).
const SSH_KEY_TTL_SECS: u64 = 30;

/// Locations of the credential files for a single user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialPaths {
    pub ssh_key: PathBuf,
    pub token: PathBuf,
}

/// Loaded credentials for a user. The `token` field MUST never be logged.
#[derive(Clone)]
pub struct Credentials {
    pub ssh_key_path: PathBuf,
    pub token: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("ssh_key_path", &self.ssh_key_path)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Errors raised when loading credentials.
#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("ssh key not found at {0}")]
    KeyNotFound(PathBuf),
    #[error("token not found at {0}")]
    TokenNotFound(PathBuf),
    #[error("ssh key {path} has permissive mode {mode:#o}, required {REQUIRED_MODE:#o}")]
    PermissiveSshKey { path: PathBuf, mode: u32 },
    #[error("token {path} has permissive mode {mode:#o}, required {REQUIRED_MODE:#o}")]
    PermissiveToken { path: PathBuf, mode: u32 },
    #[error("io error reading credential: {0}")]
    IoError(#[from] io::Error),
    #[error("invalid user name {0:?}")]
    InvalidUser(String),
    #[error("failed to start ssh-agent: {0}")]
    AgentStartFailed(String),
}

/// Returns the root credentials directory.
pub fn credentials_dir() -> PathBuf {
    PathBuf::from(CREDENTIALS_ROOT)
}

/// Returns the credential file paths for `user` rooted at the default
/// credentials directory. Does not check existence.
pub fn credential_paths(user: &str) -> Result<CredentialPaths, CredentialError> {
    credential_paths_in(&credentials_dir(), user)
}

/// Returns the credential file paths for `user` rooted at `base`. Does not
/// check existence. Rejects user names that contain path separators or
/// parent-directory components, since they could escape the credentials root.
pub fn credential_paths_in(base: &Path, user: &str) -> Result<CredentialPaths, CredentialError> {
    if user.is_empty() || user.contains('/') || user.contains('\\') || user == "." || user == ".." {
        return Err(CredentialError::InvalidUser(user.to_string()));
    }
    let user_dir = base.join(user);
    Ok(CredentialPaths {
        ssh_key: user_dir.join(SSH_KEY_FILE),
        token: user_dir.join(TOKEN_FILE),
    })
}

/// Loads credential files for `user` from the default credentials directory,
/// verifying that each file exists and has mode `0o600`.
pub fn load_credentials(user: &str) -> Result<Credentials, CredentialError> {
    load_credentials_from(&credentials_dir(), user)
}

/// Loads credential files for `user` from `base`. Used in tests to point at a
/// temp directory.
pub fn load_credentials_from(base: &Path, user: &str) -> Result<Credentials, CredentialError> {
    tracing::debug!(user = %user, "loading credentials");
    let paths = credential_paths_in(base, user)?;

    verify_ssh_key(&paths.ssh_key)?;
    let token = read_token(&paths.token)?;

    Ok(Credentials {
        ssh_key_path: paths.ssh_key,
        token,
    })
}

fn verify_ssh_key(path: &Path) -> Result<(), CredentialError> {
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(CredentialError::KeyNotFound(path.to_path_buf()));
        }
        Err(err) => return Err(CredentialError::IoError(err)),
    };
    let mode = metadata.permissions().mode() & PERMISSION_MASK;
    if mode != REQUIRED_MODE {
        return Err(CredentialError::PermissiveSshKey {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

fn read_token(path: &Path) -> Result<String, CredentialError> {
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(CredentialError::TokenNotFound(path.to_path_buf()));
        }
        Err(err) => return Err(CredentialError::IoError(err)),
    };
    let mode = metadata.permissions().mode() & PERMISSION_MASK;
    if mode != REQUIRED_MODE {
        return Err(CredentialError::PermissiveToken {
            path: path.to_path_buf(),
            mode,
        });
    }
    let raw = fs::read_to_string(path)?;
    Ok(raw.trim_end_matches(['\n', '\r']).to_string())
}

/// Injects `GIT_CONFIG_*` vars that set `safe.directory = *` so git accepts
/// repositories owned by a different user. Required when the broker (running as
/// the `ghbrk` system user) operates on repos owned by the calling user.
fn git_safe_dir_env() -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), "safe.directory".to_string()),
        ("GIT_CONFIG_VALUE_0".to_string(), "*".to_string()),
    ]
}

/// Builds env vars for an HTTPS git operation. Returns the env vars and the
/// `tempfile::NamedTempFile` holding the askpass script. The caller must keep
/// the script alive for the duration of the git invocation.
pub struct HttpsGitEnv {
    pub vars: Vec<(String, String)>,
    pub askpass_script: tempfile::NamedTempFile,
}

/// Builds the HTTPS git env: writes a temporary askpass script that prints the
/// token and points `GIT_ASKPASS` at it. The returned struct must outlive the
/// git invocation; dropping it removes the script from disk.
pub fn https_git_env(creds: &Credentials) -> Result<HttpsGitEnv, CredentialError> {
    let mut script = tempfile::Builder::new()
        .prefix("ghbrk-askpass-")
        .suffix(".sh")
        .tempfile()?;
    {
        use std::io::Write;
        writeln!(script, "#!/bin/sh")?;
        writeln!(script, "printf '%s' \"$GHBRK_TOKEN\"")?;
    }
    let mut perms = fs::metadata(script.path())?.permissions();
    perms.set_mode(0o700);
    fs::set_permissions(script.path(), perms)?;

    let mut vars = vec![
        (
            "GIT_ASKPASS".to_string(),
            script.path().display().to_string(),
        ),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("GHBRK_TOKEN".to_string(), creds.token.clone()),
    ];
    vars.extend(git_safe_dir_env());
    Ok(HttpsGitEnv {
        vars,
        askpass_script: script,
    })
}

/// RAII guard for a per-operation ssh-agent process and its temp directory.
///
/// Dropping this kills the agent and removes the temp dir, so key material
/// (in-agent memory) and the socket are cleaned up immediately after the git
/// invocation completes.
pub struct SshAgentHandle {
    pub child: std::process::Child,
    pub temp_dir: std::path::PathBuf,
}

impl Drop for SshAgentHandle {
    fn drop(&mut self) {
        // `kill()` sends SIGKILL but does not reap the zombie; the following
        // `wait()` reaps it so the agent does not linger as a defunct child
        // for the daemon's lifetime.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

/// Resolves the `ghbrk-clients` group GID, mapping every failure mode onto
/// `AgentStartFailed`. Unlike the broker's best-effort socket chgrp, the agent
/// escrow *requires* the group: without it, privilege-dropped git children
/// could not connect to the agent socket, so a missing group is a hard error.
fn client_group_gid() -> Result<nix::unistd::Gid, CredentialError> {
    match Group::from_name(CLIENT_GROUP_NAME) {
        Ok(Some(group)) => Ok(group.gid),
        Ok(None) => Err(CredentialError::AgentStartFailed(format!(
            "group {CLIENT_GROUP_NAME} not found"
        ))),
        Err(err) => Err(CredentialError::AgentStartFailed(format!(
            "looking up group {CLIENT_GROUP_NAME} failed: {err}"
        ))),
    }
}

/// Starts a per-operation `ssh-agent`, loads the user's key into it with a
/// bounded TTL, and returns the env vars a git child needs (`SSH_AUTH_SOCK`
/// plus `safe.directory`) together with an [`SshAgentHandle`] that tears the
/// agent down on drop.
///
/// Security model: the key file is `0600 ghbrk:ghbrk` and is read only here, in
/// the daemon. The key never reaches the privilege-dropped git child; instead
/// the child connects to the agent over a `0660 ghbrk:ghbrk-clients` socket and
/// the agent performs the signing. The agent and its socket are confined to a
/// `0710 ghbrk:ghbrk-clients` temp dir so only `ghbrk-clients` members can
/// traverse into it.
pub async fn start_ssh_agent(
    creds: &Credentials,
) -> Result<(Vec<(String, String)>, SshAgentHandle), CredentialError> {
    let gid = client_group_gid()?;

    // Take ownership of the temp dir immediately: its lifetime is managed by
    // `SshAgentHandle::drop`, not by `TempDir`'s own drop. On every error path
    // before the agent spawns we must clean it up explicitly.
    let temp_dir: std::path::PathBuf = tempfile::Builder::new()
        .prefix("ghbrk-ssh-agent-")
        .tempdir()
        .map_err(|err| CredentialError::AgentStartFailed(format!("creating temp dir: {err}")))?
        // `keep()` is tempfile's non-deprecated equivalent of `into_path()`:
        // it dissolves the `TempDir` and returns the owned path *without*
        // scheduling removal, so lifetime is ours (via `SshAgentHandle`).
        .keep();

    // 0710 ghbrk:ghbrk-clients: owner has full access, group may only traverse
    // (--x) to reach the socket, others have nothing.
    if let Err(err) = std::fs::set_permissions(&temp_dir, std::fs::Permissions::from_mode(0o710)) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(CredentialError::AgentStartFailed(format!(
            "chmod temp dir: {err}"
        )));
    }
    if let Err(err) = chown(&temp_dir, None, Some(gid)) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(CredentialError::AgentStartFailed(format!(
            "chgrp temp dir: {err}"
        )));
    }

    let socket_path = temp_dir.join("agent.sock");

    // Spawn the agent with `-D` so it runs in the foreground and remains our
    // direct child (default behaviour daemonizes, which would orphan it).
    // std (not tokio) `Command` is required so the resulting `Child` can be
    // reaped synchronously from `SshAgentHandle::drop`.
    let agent_child = match std::process::Command::new("ssh-agent")
        .arg("-D")
        .arg("-a")
        .arg(&socket_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(CredentialError::AgentStartFailed(format!(
                "spawning ssh-agent: {err}"
            )));
        }
    };

    // Wrap the child in the handle now so that every error return past this
    // point tears the agent down (kill + reap) and removes the temp dir via
    // the `Drop` impl — no manual cleanup needed below.
    let handle = SshAgentHandle {
        child: agent_child,
        temp_dir,
    };

    // Poll for the socket to appear: up to 20 * 50ms = 1s.
    let mut ready = false;
    for _ in 0..20 {
        if socket_path.exists() {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !ready {
        // `handle` drops here, killing the agent and removing the temp dir.
        return Err(CredentialError::AgentStartFailed(
            "ssh-agent socket did not appear within 1s".to_string(),
        ));
    }

    // Load the key into the agent with a bounded TTL. `ssh-add` reads the key
    // (the daemon can, since it is `0600 ghbrk:ghbrk`) and hands it to the
    // agent over the socket we just created.
    let add_status = std::process::Command::new("ssh-add")
        .arg("-t")
        .arg(SSH_KEY_TTL_SECS.to_string())
        .arg(&creds.ssh_key_path)
        .env("SSH_AUTH_SOCK", &socket_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match add_status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            // `handle` drops here -> agent killed, temp dir removed.
            return Err(CredentialError::AgentStartFailed(format!(
                "ssh-add exited with status {status}"
            )));
        }
        Err(err) => {
            return Err(CredentialError::AgentStartFailed(format!(
                "running ssh-add: {err}"
            )));
        }
    }

    // Widen the socket to 0660 and chgrp it to ghbrk-clients so privilege-
    // dropped git children (members of that group) may connect. ssh-agent
    // created it 0600 ghbrk:ghbrk.
    if let Err(err) = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o660))
    {
        return Err(CredentialError::AgentStartFailed(format!(
            "chmod agent socket: {err}"
        )));
    }
    if let Err(err) = chown(&socket_path, None, Some(gid)) {
        return Err(CredentialError::AgentStartFailed(format!(
            "chgrp agent socket: {err}"
        )));
    }

    let mut env_vars = vec![(
        "SSH_AUTH_SOCK".to_string(),
        socket_path.to_string_lossy().into_owned(),
    )];
    env_vars.extend(git_safe_dir_env());

    Ok((env_vars, handle))
}

/// Builds env vars for a `gh` invocation.
///
/// `GH_TOKEN` is always supplied from the user's credentials. The executor
/// clears the parent environment before spawning `gh`, so a `GH_HOST` set on
/// the daemon (e.g. to target a GitHub Enterprise host or an integration mock)
/// would otherwise be dropped. It is forwarded here when present.
pub fn gh_env(creds: &Credentials) -> Vec<(String, String)> {
    let mut env = vec![("GH_TOKEN".to_string(), creds.token.clone())];
    // gh calls os.UserHomeDir() to locate its config dir. Forward the daemon's
    // HOME so gh does not fall back to the ghbrk system user's passwd entry
    // (which is /nonexistent for daemon accounts created with --no-create-home).
    if let Ok(home) = std::env::var("HOME") {
        env.push(("HOME".to_string(), home));
    }
    if let Ok(host) = std::env::var("GH_HOST") {
        if !host.is_empty() {
            // For a non-github.com host, `gh` reads the token from
            // GH_ENTERPRISE_TOKEN and ignores GH_TOKEN, so supply both.
            if host != "github.com" {
                env.push(("GH_ENTERPRISE_TOKEN".to_string(), creds.token.clone()));
            }
            env.push(("GH_HOST".to_string(), host));
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tracing::subscriber;
    use tracing_subscriber::fmt::MakeWriter;

    fn write_file(path: &Path, contents: &str, mode: u32) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms).unwrap();
    }

    fn write_user_creds(base: &Path, user: &str, key: &str, token: &str, mode: u32) {
        write_file(&base.join(user).join(SSH_KEY_FILE), key, mode);
        write_file(&base.join(user).join(TOKEN_FILE), token, mode);
    }

    #[test]
    fn loads_when_modes_are_0600() {
        let dir = TempDir::new().unwrap();
        write_user_creds(dir.path(), "alice", "KEYDATA", "TOKENDATA", 0o600);
        let creds = load_credentials_from(dir.path(), "alice").unwrap();
        assert!(creds.ssh_key_path.ends_with("alice/id_rsa"));
        assert_eq!(creds.token, "TOKENDATA");
    }

    #[test]
    fn trims_trailing_newline_from_token() {
        let dir = TempDir::new().unwrap();
        write_user_creds(dir.path(), "alice", "KEYDATA", "ghp_abc\n", 0o600);
        let creds = load_credentials_from(dir.path(), "alice").unwrap();
        assert_eq!(creds.token, "ghp_abc");
    }

    #[test]
    fn missing_ssh_key_returns_key_not_found() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("alice/token"), "T", 0o600);
        let err = load_credentials_from(dir.path(), "alice").unwrap_err();
        assert!(matches!(err, CredentialError::KeyNotFound(_)));
    }

    #[test]
    fn missing_token_returns_token_not_found() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("alice/id_rsa"), "K", 0o600);
        let err = load_credentials_from(dir.path(), "alice").unwrap_err();
        assert!(matches!(err, CredentialError::TokenNotFound(_)));
    }

    #[test]
    fn permissive_ssh_key_mode_rejected() {
        let dir = TempDir::new().unwrap();
        write_user_creds(dir.path(), "alice", "K", "T", 0o644);
        let err = load_credentials_from(dir.path(), "alice").unwrap_err();
        match err {
            CredentialError::PermissiveSshKey { mode, .. } => assert_eq!(mode, 0o644),
            other => panic!("expected PermissiveSshKey, got {other:?}"),
        }
    }

    #[test]
    fn permissive_token_mode_rejected() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("alice/id_rsa"), "K", 0o600);
        write_file(&dir.path().join("alice/token"), "T", 0o604);
        let err = load_credentials_from(dir.path(), "alice").unwrap_err();
        match err {
            CredentialError::PermissiveToken { mode, .. } => assert_eq!(mode, 0o604),
            other => panic!("expected PermissiveToken, got {other:?}"),
        }
    }

    #[test]
    fn group_readable_token_rejected() {
        let dir = TempDir::new().unwrap();
        write_file(&dir.path().join("alice/id_rsa"), "K", 0o600);
        write_file(&dir.path().join("alice/token"), "T", 0o640);
        let err = load_credentials_from(dir.path(), "alice").unwrap_err();
        assert!(matches!(
            err,
            CredentialError::PermissiveToken { mode: 0o640, .. }
        ));
    }

    #[test]
    fn user_with_path_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let err = load_credentials_from(dir.path(), "../etc").unwrap_err();
        assert!(matches!(err, CredentialError::InvalidUser(_)));
        let err2 = load_credentials_from(dir.path(), "alice/bob").unwrap_err();
        assert!(matches!(err2, CredentialError::InvalidUser(_)));
        let err3 = load_credentials_from(dir.path(), "").unwrap_err();
        assert!(matches!(err3, CredentialError::InvalidUser(_)));
    }

    #[test]
    fn ssh_env_removed_git_safe_dir_still_works() {
        let env = git_safe_dir_env();
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(map["GIT_CONFIG_COUNT"], "1");
        assert_eq!(map["GIT_CONFIG_KEY_0"], "safe.directory");
        assert_eq!(map["GIT_CONFIG_VALUE_0"], "*");
        // GIT_SSH_COMMAND injection removed — key escrow now via SSH_AUTH_SOCK
        assert!(
            !map.contains_key("GIT_SSH_COMMAND"),
            "ssh_env was removed; GIT_SSH_COMMAND must not appear in safe.directory env"
        );
    }

    /// Serializes tests that mutate the process-global `GH_HOST` env var.
    static GH_HOST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn gh_env_sets_gh_token() {
        let _guard = GH_HOST_LOCK.lock().unwrap();
        std::env::remove_var("GH_HOST");
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_secret".into(),
        };
        let env = gh_env(&creds);
        assert_eq!(
            env,
            vec![("GH_TOKEN".to_string(), "ghp_secret".to_string())]
        );
    }

    #[test]
    fn gh_env_forwards_gh_host_when_set() {
        let _guard = GH_HOST_LOCK.lock().unwrap();
        std::env::set_var("GH_HOST", "mock-github");
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_secret".into(),
        };
        let env = gh_env(&creds);
        std::env::remove_var("GH_HOST");
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(map.get("GH_TOKEN").map(String::as_str), Some("ghp_secret"));
        assert_eq!(map.get("GH_HOST").map(String::as_str), Some("mock-github"));
    }

    #[test]
    fn gh_env_sets_enterprise_token_for_non_github_host() {
        // `gh` ignores GH_TOKEN when GH_HOST points at a non-github.com host
        // and reads GH_ENTERPRISE_TOKEN instead. The broker must supply both.
        let _guard = GH_HOST_LOCK.lock().unwrap();
        std::env::set_var("GH_HOST", "mock-github");
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_secret".into(),
        };
        let env = gh_env(&creds);
        std::env::remove_var("GH_HOST");
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();
        assert_eq!(
            map.get("GH_ENTERPRISE_TOKEN").map(String::as_str),
            Some("ghp_secret")
        );
    }

    #[test]
    fn gh_env_omits_enterprise_token_for_github_com() {
        let _guard = GH_HOST_LOCK.lock().unwrap();
        std::env::set_var("GH_HOST", "github.com");
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_secret".into(),
        };
        let env = gh_env(&creds);
        std::env::remove_var("GH_HOST");
        assert!(env.iter().all(|(k, _)| k != "GH_ENTERPRISE_TOKEN"));
    }

    #[test]
    fn gh_env_omits_gh_host_when_unset() {
        let _guard = GH_HOST_LOCK.lock().unwrap();
        std::env::remove_var("GH_HOST");
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_secret".into(),
        };
        let env = gh_env(&creds);
        assert!(env.iter().all(|(k, _)| k != "GH_HOST"));
    }

    #[test]
    fn gh_env_forwards_home_when_set() {
        let _guard = GH_HOST_LOCK.lock().unwrap();
        std::env::remove_var("GH_HOST");
        std::env::set_var("HOME", "/run/ghbrk");
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_secret".into(),
        };
        let env = gh_env(&creds);
        std::env::remove_var("HOME");
        assert!(
            env.iter().any(|(k, v)| k == "HOME" && v == "/run/ghbrk"),
            "HOME must be forwarded to gh child processes"
        );
    }

    #[test]
    fn gh_env_omits_home_when_unset() {
        let _guard = GH_HOST_LOCK.lock().unwrap();
        std::env::remove_var("GH_HOST");
        std::env::remove_var("HOME");
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_secret".into(),
        };
        let env = gh_env(&creds);
        assert!(
            env.iter().all(|(k, _)| k != "HOME"),
            "HOME must be absent when not set in daemon environment"
        );
    }

    #[test]
    fn https_git_env_sets_askpass_and_token_indirection() {
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x"),
            token: "ghp_https_secret".into(),
        };
        let env = https_git_env(&creds).unwrap();
        let map: std::collections::HashMap<_, _> = env.vars.iter().cloned().collect();
        assert!(map.contains_key("GIT_ASKPASS"));
        assert_eq!(
            map.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            map.get("GHBRK_TOKEN").map(String::as_str),
            Some("ghp_https_secret")
        );

        let askpass_path = PathBuf::from(map.get("GIT_ASKPASS").unwrap());
        assert!(askpass_path.exists());
        let mode = fs::metadata(&askpass_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let body = fs::read_to_string(&askpass_path).unwrap();
        assert!(
            !body.contains("ghp_https_secret"),
            "token must not be embedded in script body"
        );
        assert!(body.contains("GHBRK_TOKEN"));
    }

    #[test]
    fn debug_format_redacts_token() {
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/x/key"),
            token: "ghp_supersecret_TOKENVALUE".into(),
        };
        let s = format!("{:?}", creds);
        assert!(!s.contains("ghp_supersecret_TOKENVALUE"));
        assert!(s.contains("redacted"));
    }

    /// Captures all tracing output produced inside `f` and returns it as a string.
    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for CapturedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CapturedWriter {
        type Writer = CapturedWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_tracing<F: FnOnce()>(f: F) -> String {
        let buf = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        subscriber::with_default(subscriber, f);
        let bytes = buf.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn token_never_appears_in_tracing_output() {
        let dir = TempDir::new().unwrap();
        let secret = "ghp_VERY_SECRET_TOKEN_VALUE_xyz123";
        write_user_creds(dir.path(), "alice", "KEY", secret, 0o600);

        let dir_path = dir.path().to_path_buf();
        let captured = capture_tracing(|| {
            let creds = load_credentials_from(&dir_path, "alice").unwrap();
            // Touch every helper that consumes credentials. None of them may log
            // the token contents.
            let _ = gh_env(&creds);
            tracing::debug!(?creds, "credentials loaded");
        });

        assert!(
            !captured.contains(secret),
            "token must never appear in tracing output: captured={captured:?}"
        );
    }
}
