use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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

/// Builds env vars to inject for an SSH-based git operation.
///
/// Sets `GIT_SSH_COMMAND` so git uses the configured key and accepts new host
/// keys on first contact.
pub fn ssh_env(creds: &Credentials) -> Vec<(String, String)> {
    let key = creds.ssh_key_path.display();
    vec![(
        "GIT_SSH_COMMAND".to_string(),
        format!("ssh -i {key} -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/dev/null"),
    )]
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

    let vars = vec![
        (
            "GIT_ASKPASS".to_string(),
            script.path().display().to_string(),
        ),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("GHBRK_TOKEN".to_string(), creds.token.clone()),
    ];
    Ok(HttpsGitEnv {
        vars,
        askpass_script: script,
    })
}

/// Builds env vars for a `gh` invocation.
///
/// `GH_TOKEN` is always supplied from the user's credentials. The executor
/// clears the parent environment before spawning `gh`, so a `GH_HOST` set on
/// the daemon (e.g. to target a GitHub Enterprise host or an integration mock)
/// would otherwise be dropped. It is forwarded here when present.
pub fn gh_env(creds: &Credentials) -> Vec<(String, String)> {
    let mut env = vec![("GH_TOKEN".to_string(), creds.token.clone())];
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
    fn ssh_env_uses_key_path_and_strict_host_key_accept_new() {
        let creds = Credentials {
            ssh_key_path: PathBuf::from("/etc/ghbrk/credentials/alice/id_rsa"),
            token: "secret".into(),
        };
        let env = ssh_env(&creds);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "GIT_SSH_COMMAND");
        assert_eq!(
            env[0].1,
            "ssh -i /etc/ghbrk/credentials/alice/id_rsa -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=/dev/null"
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
            let _ = ssh_env(&creds);
            let _ = gh_env(&creds);
            tracing::debug!(?creds, "credentials loaded");
        });

        assert!(
            !captured.contains(secret),
            "token must never appear in tracing output: captured={captured:?}"
        );
    }
}
