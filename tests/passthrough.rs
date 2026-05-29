use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use ghbrk::audit::AuditLogger;
use ghbrk::broker::{run_broker, username_for_uid, BrokerConfig};
use ghbrk::policy::Policy;
use tempfile::TempDir;
use tokio::runtime::Runtime;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

const REAL_GIT: &str = "/usr/bin/git";
const TOKEN: &str = "ghp_passthrough_marker_77";

fn current_user() -> String {
    username_for_uid(nix::unistd::Uid::current()).expect("current process must have a username")
}

fn write_mode(path: &Path, contents: &str, mode: u32) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// An in-process broker bound to a temp socket. The child `gh` it spawns
/// inherits this process's PATH (after `env_clear`), so install the stub `gh`
/// on PATH before constructing the daemon.
struct MinimalDaemon {
    socket_path: PathBuf,
    audit_path: PathBuf,
    _creds_root: TempDir,
    _socket_dir: TempDir,
    _audit_dir: TempDir,
    handle: Option<tokio::task::JoinHandle<()>>,
    _rt: Arc<Runtime>,
}

impl MinimalDaemon {
    fn new(creds_root: TempDir) -> Self {
        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("broker.sock");
        let audit_dir = tempfile::tempdir().unwrap();
        let audit_path = audit_dir.path().join("audit.log");

        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        );

        let logger = Arc::new(AuditLogger::new(&audit_path).unwrap());
        let config = BrokerConfig {
            socket_path: socket_path.clone(),
            // Deny-everything policy: passthrough must bypass policy.
            policy: Policy::from_yaml("rules: []").unwrap(),
            audit_logger: logger,
            credentials_root: Some(creds_root.path().to_path_buf()),
        };

        let sp = socket_path.clone();
        let handle = rt.spawn(async move {
            let _ = run_broker(config).await;
        });

        for _ in 0..500 {
            if sp.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        MinimalDaemon {
            socket_path,
            audit_path,
            _creds_root: creds_root,
            _socket_dir: socket_dir,
            _audit_dir: audit_dir,
            handle: Some(handle),
            _rt: rt,
        }
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for MinimalDaemon {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// Create a credentials root with the current user's SSH key and token.
fn setup_creds() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let user_dir = tmp.path().join(current_user());
    fs::create_dir_all(&user_dir).unwrap();
    write_mode(&user_dir.join("id_rsa"), "dummy-key", 0o600);
    write_mode(&user_dir.join("token"), TOKEN, 0o600);
    tmp
}

/// Serializes the process-global `PATH` mutation done by `install_stub_gh` so
/// parallel tests do not race on which `gh` stub is first on PATH.
static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Install a stub `gh` on PATH (prepended) so the in-process broker resolves
/// it when it spawns the child. The stub prints the reassembled argv and the
/// injected `GH_TOKEN`, then exits with code 7 for `--version` (to prove exit
/// codes propagate) and 0 otherwise. Returns the holding TempDir; PATH stays
/// pointed at it for the lifetime of the returned dir.
fn install_stub_gh() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("gh");
    write_mode(
        &stub,
        "#!/bin/sh\n\
         printf 'argv=%s GH_TOKEN=%s' \"$*\" \"$GH_TOKEN\"\n\
         if [ \"$1\" = \"--version\" ]; then exit 7; fi\n\
         exit 0\n",
        0o755,
    );
    let prev = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", dir.path().display(), prev));
    dir
}

/// Run the `ghbrk` `gh` symlink against the daemon's socket with `GH_TOKEN`
/// removed from the child's inherited env (it must come from the broker).
fn run_gh(socket: &Path, args: &[&str]) -> std::process::Output {
    let env = TestEnv::new();
    let gh_sym = env.make_symlink("gh");
    let config_path = env.write_config(REAL_GIT, "/usr/bin/gh");
    Command::new(&gh_sym)
        .args(args)
        .env("GHBRK_CONFIG", &config_path)
        .env("GHBRK_SOCKET", socket)
        .env_remove("GH_TOKEN")
        .output()
        .expect("invoke gh symlink")
}

struct TestEnv {
    dir: TempDir,
}

impl TestEnv {
    fn new() -> Self {
        TestEnv {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn write_config(&self, real_git: &str, real_gh: &str) -> PathBuf {
        let config_path = self.dir.path().join("ghbrk-config.yaml");
        let content = format!("real_git: {real_git}\nreal_gh: {real_gh}\n");
        fs::write(&config_path, content).expect("write config");
        config_path
    }

    fn make_symlink(&self, name: &str) -> PathBuf {
        let link = self.dir.path().join(name);
        std::os::unix::fs::symlink(bin(), &link).expect("create symlink");
        link
    }
}

fn init_git_repo(dir: &std::path::Path) {
    let runs: &[&[&str]] = &[
        &["init", "-b", "main"],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "Test User"],
        &["commit", "--allow-empty", "-m", "init"],
    ];
    for args in runs {
        let out = Command::new(REAL_GIT)
            .args(args.iter().copied())
            .current_dir(dir)
            .output()
            .expect("real git command");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn git_status_passes_through() {
    let env = TestEnv::new();
    let repo_dir = env.dir.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo(&repo_dir);

    let config_path = env.write_config(REAL_GIT, "/usr/bin/gh");
    let git_sym = env.make_symlink("git");

    let out = Command::new(&git_sym)
        .arg("status")
        .current_dir(&repo_dir)
        .env("GHBRK_CONFIG", &config_path)
        .output()
        .expect("invoke git symlink");

    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("On branch") || stdout.contains("nothing to commit"),
        "expected git status output, got: {stdout}"
    );
}

#[test]
fn git_push_takes_broker_path() {
    let env = TestEnv::new();
    let repo_dir = env.dir.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo(&repo_dir);

    let fake_origin = env.dir.path().join("fake-origin.git");
    fs::create_dir_all(&fake_origin).unwrap();
    let init_bare = Command::new(REAL_GIT)
        .args(["init", "--bare", fake_origin.to_str().unwrap()])
        .output()
        .expect("git init --bare");
    assert!(init_bare.status.success());

    let add_remote = Command::new(REAL_GIT)
        .args(["remote", "add", "origin", fake_origin.to_str().unwrap()])
        .current_dir(&repo_dir)
        .output()
        .expect("git remote add");
    assert!(add_remote.status.success());

    let config_path = env.write_config(REAL_GIT, "/usr/bin/gh");
    let git_sym = env.make_symlink("git");

    let nonexistent_socket = env.dir.path().join("no-such.sock");

    let out = Command::new(&git_sym)
        .args(["push", "origin", "main"])
        .current_dir(&repo_dir)
        .env("GHBRK_CONFIG", &config_path)
        .env("GHBRK_SOCKET", &nonexistent_socket)
        .output()
        .expect("invoke git push via symlink");

    assert_ne!(
        out.status.code(),
        Some(0),
        "expected non-zero exit for broker path with no socket"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("connect")
            || stderr.contains("socket")
            || stderr.contains("ghbrk")
            || stderr.contains("No such file")
            || stderr.contains("Connection refused"),
        "expected broker connection error in stderr, got: {stderr}"
    );
}

/// After routing `gh` through the broker, a passthrough invocation's exit code
/// must propagate from the executed `gh` back through the broker and shim.
#[test]
fn passthrough_propagates_exit_code() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _stub = install_stub_gh();
    let daemon = MinimalDaemon::new(setup_creds());

    let out = run_gh(daemon.socket_path(), &["--version"]);

    assert_eq!(
        out.status.code(),
        Some(7),
        "expected exit code 7 propagated through broker, got: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn passthrough_missing_binary_errors() {
    let env = TestEnv::new();

    let config_path = env.write_config("/nonexistent/fake-git", "/nonexistent/fake-gh");
    let git_sym = env.make_symlink("git");

    let out = Command::new(&git_sym)
        .arg("status")
        .env("GHBRK_CONFIG", &config_path)
        .env("GHBRK_SOCKET", "/nonexistent/sock")
        .output()
        .expect("run ghbrk");

    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got: {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ghbrk: failed to exec"),
        "expected exec error in stderr, got: {stderr}"
    );
}

/// `gh auth status` is a passthrough command, but after the fix it must now go
/// through the broker (not direct exec) and receive an injected `GH_TOKEN`. An
/// audit record with `decision=passthrough` must be written.
#[test]
fn gh_auth_status_routes_to_broker_with_token() {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _stub = install_stub_gh();
    let daemon = MinimalDaemon::new(setup_creds());

    let out = run_gh(daemon.socket_path(), &["auth", "status"]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0 through broker, got: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("GH_TOKEN={TOKEN}")),
        "stub gh did not receive injected GH_TOKEN; got: {stdout}"
    );

    let audit_body = fs::read_to_string(&daemon.audit_path).unwrap();
    assert!(
        audit_body.contains(r#""decision":"passthrough""#),
        "expected passthrough decision in audit log; got: {audit_body}"
    );
}
