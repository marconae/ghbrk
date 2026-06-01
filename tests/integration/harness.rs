//! End-to-end integration harness.
//!
//! These tests stand up a real SSH-accessible bare git server in a Docker
//! Compose project, exercise the full broker / shim / executor pipeline, and
//! assert outcomes on the working tree, the remote refs, and the audit log.
//!
//! ## Prerequisites
//!
//! - Docker engine reachable from the test runner with the `compose` plugin
//!   (`docker compose version` succeeds).
//! - `ssh-keygen`, `git`, and `ssh-keyscan` available on the test host.
//! - TCP port 2222 free on the host.
//!
//! When Docker is unavailable, every test prints a skip message and returns
//! successfully so that `cargo test --test harness` does not break developer
//! workflows on machines without Docker.
//!
//! ## Concurrency
//!
//! All tests share a single Docker Compose project (one bare repo, one host
//! port). They MUST run serially — invoke them with
//! `cargo test --test harness -- --test-threads=1`. A module-level `Mutex`
//! also guards against accidental parallel runs inside the same process.
//!
//! ## URL routing
//!
//! The broker's resolver only accepts `github.com` URLs, but the harness
//! container is reachable at `ssh://git@localhost:2222/...`. Bridging the two
//! is done by a per-test `git` wrapper script placed on the daemon's PATH:
//! - The shim is invoked with the canonical URL
//!   `ssh://git@github.com/test-org/test.git`.
//! - The resolver parses that as `org=test-org`, `repo=test`, scheme=Ssh.
//! - The wrapper rewrites the URL via `-c url.<harness>.insteadOf=...` and
//!   appends the harness port to `GIT_SSH_COMMAND` before exec'ing real git.
//! - git stores the original (un-rewritten) URL in `.git/config`, so the
//!   resolver runs cleanly on subsequent push/fetch invocations as well.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const HOST_PORT: u16 = 2222;
const REAL_GIT: &str = "/usr/bin/git";
const CONTAINER_NAME: &str = "ghbrk-it-git-server";
const DEVENV_CONTAINER: &str = "ghbrk-it-devenv";
const REMOTE_ORG: &str = "test-org";
const HARNESS_GIT_URL: &str = "ssh://git@github.com/test-org/test.git";

/// Synthetic token planted in the credentials store for the broker `gh api`
/// tests. The mock GitHub API accepts any non-empty bearer token.
const FAKE_TOKEN: &str = "test-fake-token-value";

/// Hostname the mock GitHub service is reachable at on the Docker network.
const MOCK_GH_HOST: &str = "mock-github";

/// Path of the static `ghbrk` binary copied into the `devenv` container, and
/// the per-test daemon socket inside that container.
const CONTAINER_BIN: &str = "/usr/local/bin/ghbrk";
const CONTAINER_SOCKET: &str = "/tmp/ghbrk-test.sock";

/// Serializes test-level access to the shared Docker Compose project.
static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

fn compose_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/integration")
}

fn docker_available() -> bool {
    Command::new("docker")
        .args(["compose", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn skip_if_no_docker(name: &str) -> bool {
    if !docker_available() {
        eprintln!("[harness] skipping {name}: docker compose unavailable");
        true
    } else {
        false
    }
}

/// RAII guard that runs `docker compose down -v` in the integration directory
/// when dropped. Constructed by [`start_compose`].
struct ComposeGuard {
    compose_dir: PathBuf,
}

impl ComposeGuard {
    fn down(&self) {
        let _ = Command::new("docker")
            .args(["compose", "down", "-v", "--remove-orphans"])
            .current_dir(&self.compose_dir)
            .output();
    }
}

impl Drop for ComposeGuard {
    fn drop(&mut self) {
        self.down();
    }
}

/// Brings the harness compose project up and waits for SSH to become
/// reachable on `HOST_PORT`. Returns a guard that tears the project down on
/// drop.
fn start_compose() -> ComposeGuard {
    // Best-effort cleanup of any leftover containers from a previous run.
    let _ = Command::new("docker")
        .args(["compose", "down", "-v", "--remove-orphans"])
        .current_dir(compose_dir())
        .output();

    for name in &[CONTAINER_NAME, DEVENV_CONTAINER, "ghbrk-it-mock-github"] {
        let _ = Command::new("docker").args(["rm", "-f", name]).output();
    }

    let up = Command::new("docker")
        .args(["compose", "up", "-d", "--build"])
        .current_dir(compose_dir())
        .output()
        .expect("failed to invoke docker compose up");
    assert!(
        up.status.success(),
        "docker compose up failed: stdout={} stderr={}",
        String::from_utf8_lossy(&up.stdout),
        String::from_utf8_lossy(&up.stderr)
    );

    let guard = ComposeGuard {
        compose_dir: compose_dir(),
    };
    wait_for_ssh(HOST_PORT, Duration::from_secs(120));
    guard
}

fn wait_for_ssh(port: u16, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let port_arg = port.to_string();
    while Instant::now() < deadline {
        let out = Command::new("ssh-keyscan")
            .args(["-T", "2", "-p", &port_arg, "localhost"])
            .output();
        if let Ok(o) = out {
            if o.status.success() && !o.stdout.is_empty() {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("ssh server on port {port} did not become reachable within {timeout:?}");
}

/// Per-run SSH key material plus the rendered remote URL.
struct SshHarness {
    /// Holds the credentials directory tree alive for the lifetime of the test.
    creds_dir: TempDir,
    private_key_path: PathBuf,
    #[allow(dead_code)]
    public_key: String,
    #[allow(dead_code)]
    user: String,
}

impl SshHarness {
    /// Generates an ed25519 keypair, writes the private key to
    /// `<creds_dir>/<user>/id_rsa` with mode 0600, writes a placeholder token
    /// (the broker insists every credential file exists), and uploads the
    /// public key into the running container's `authorized_keys`.
    fn setup(user: &str) -> Self {
        let creds_dir = tempfile::tempdir().expect("tempdir for creds");
        let user_dir = creds_dir.path().join(user);
        fs::create_dir_all(&user_dir).expect("mkdir user creds");

        let private_key_path = user_dir.join("id_rsa");
        let token_path = user_dir.join("token");

        // Generate ed25519 keypair via ssh-keygen.
        let kg = Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-N",
                "",
                "-q",
                "-C",
                "ghbrk-integration",
                "-f",
            ])
            .arg(&private_key_path)
            .output()
            .expect("ssh-keygen failed to start");
        assert!(
            kg.status.success(),
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&kg.stderr)
        );

        // The broker requires the private key file to be exactly 0600.
        let mut perms = fs::metadata(&private_key_path).unwrap().permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&private_key_path, perms).unwrap();

        // Read the public key for injection into the container.
        let pub_key_path = user_dir.join("id_rsa.pub");
        let public_key = fs::read_to_string(&pub_key_path)
            .expect("read public key")
            .trim()
            .to_string();

        // The broker also requires a token file with mode 0600. The harness
        // never exercises HTTPS auth so the contents are placeholder bytes.
        {
            let mut token = fs::File::create(&token_path).unwrap();
            token.write_all(b"placeholder-token").unwrap();
        }
        let mut tperms = fs::metadata(&token_path).unwrap().permissions();
        tperms.set_mode(0o600);
        fs::set_permissions(&token_path, tperms).unwrap();

        inject_authorized_key(&public_key);

        SshHarness {
            creds_dir,
            private_key_path,
            public_key,
            user: user.to_string(),
        }
    }

    fn creds_root(&self) -> &Path {
        self.creds_dir.path()
    }
}

fn inject_authorized_key(public_key: &str) {
    use std::process::Stdio;

    // Create the .ssh directory and set permissions via sh, without any
    // user-supplied data in the shell arguments.
    let mkdir = Command::new("docker")
        .args([
            "exec",
            CONTAINER_NAME,
            "sh",
            "-c",
            "install -d -m 700 -o git -g git /home/git/.ssh",
        ])
        .output()
        .expect("docker exec mkdir .ssh");
    assert!(
        mkdir.status.success(),
        "failed to create .ssh dir: stdout={} stderr={}",
        String::from_utf8_lossy(&mkdir.stdout),
        String::from_utf8_lossy(&mkdir.stderr)
    );

    // Write the public key via stdin so no shell interpolation is needed.
    let mut key_bytes = public_key.as_bytes().to_vec();
    if !key_bytes.ends_with(b"\n") {
        key_bytes.push(b'\n');
    }
    let mut child = Command::new("docker")
        .args([
            "exec",
            "-i",
            CONTAINER_NAME,
            "tee",
            "/home/git/.ssh/authorized_keys",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("docker exec tee authorized_keys");
    {
        use std::io::Write as _;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&key_bytes)
            .expect("write pubkey to stdin");
    }
    let status = child.wait().expect("wait for tee");
    assert!(status.success(), "tee authorized_keys failed: {status:?}");

    // Fix permissions so sshd accepts the file.
    let chmod = Command::new("docker")
        .args([
            "exec",
            CONTAINER_NAME,
            "sh",
            "-c",
            "chmod 600 /home/git/.ssh/authorized_keys && chown git:git /home/git/.ssh/authorized_keys",
        ])
        .output()
        .expect("docker exec chmod authorized_keys");
    assert!(
        chmod.status.success(),
        "failed to chmod authorized_keys: stdout={} stderr={}",
        String::from_utf8_lossy(&chmod.stdout),
        String::from_utf8_lossy(&chmod.stderr)
    );
}

/// The wrapper script directory + the path itself. Held by `DaemonHandle` so
/// the temp directory survives until the daemon is stopped.
struct GitWrapper {
    _dir: TempDir,
    bin_dir: PathBuf,
}

impl GitWrapper {
    /// Writes a `git` shell wrapper that bridges the resolver's GitHub URL
    /// requirement and the harness's local SSH server.
    fn install() -> Self {
        let dir = tempfile::tempdir().expect("tempdir for git wrapper");
        let bin_dir = dir.path().to_path_buf();
        let wrapper = bin_dir.join("git");
        let script = format!(
            "#!/bin/sh\n\
             # ghbrk integration test git wrapper.\n\
             GIT_SSH_COMMAND=\"${{GIT_SSH_COMMAND:-ssh}} -p {port} -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no\"\n\
             export GIT_SSH_COMMAND\n\
             exec /usr/bin/git \\\n  \
                 -c \"url.ssh://git@localhost:{port}/home/git/repos/.insteadOf=ssh://git@github.com/{org}/\" \\\n  \
                 \"$@\"\n",
            port = HOST_PORT,
            org = REMOTE_ORG,
        );
        let mut f = fs::File::create(&wrapper).expect("create wrapper script");
        f.write_all(script.as_bytes()).unwrap();
        let mut perms = fs::metadata(&wrapper).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&wrapper, perms).unwrap();
        GitWrapper { _dir: dir, bin_dir }
    }
}

/// Owned daemon child process plus the temp paths that must outlive it.
struct DaemonHandle {
    process: Child,
    socket_path: PathBuf,
    audit_log_path: PathBuf,
    _audit_dir: TempDir,
    _socket_dir: TempDir,
    _wrapper: GitWrapper,
}

impl DaemonHandle {
    /// Starts a `ghbrk daemon` child process bound to a temp socket, with the
    /// supplied policy and credentials root. Returns once the socket appears
    /// or panics on timeout.
    fn start(creds_root: &Path, policy_yaml: &str) -> Self {
        let socket_dir = tempfile::tempdir().expect("tempdir for socket");
        let audit_dir = tempfile::tempdir().expect("tempdir for audit");
        // Keep the basename short — Linux unix socket paths max out at 108
        // bytes including the null terminator.
        let socket_path = socket_dir.path().join("b.sock");
        let audit_log_path = audit_dir.path().join("audit.log");
        let policy_path = audit_dir.path().join("policy.yaml");
        fs::write(&policy_path, policy_yaml).expect("write policy");

        let wrapper = GitWrapper::install();

        // Prepend the wrapper directory to PATH so the executor's `git` lookup
        // resolves to our bridge first.
        let parent_path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string());
        let augmented_path = format!("{}:{}", wrapper.bin_dir.display(), parent_path);

        let mut cmd = Command::new(bin());
        cmd.arg("daemon")
            .env("GHBRK_SOCKET", &socket_path)
            .env("GHBRK_POLICY", &policy_path)
            .env("GHBRK_AUDIT_LOG", &audit_log_path)
            .env("GHBRK_CREDENTIALS_ROOT", creds_root)
            .env("PATH", &augmented_path);

        let process = cmd.spawn().expect("spawn ghbrk daemon");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if socket_path.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !socket_path.exists() {
            panic!(
                "ghbrk daemon did not bind {} within 5s",
                socket_path.display()
            );
        }

        DaemonHandle {
            process,
            socket_path,
            audit_log_path,
            _audit_dir: audit_dir,
            _socket_dir: socket_dir,
            _wrapper: wrapper,
        }
    }

    fn audit_lines(&self) -> Vec<String> {
        let body = fs::read_to_string(&self.audit_log_path).unwrap_or_default();
        body.lines()
            .filter(|l| !l.is_empty())
            .map(|s| s.to_string())
            .collect()
    }
}

/// Returns true when at least one audit line has the given `operation` and
/// `decision` values. Parses each line as JSON so the check is robust against
/// whitespace variations in the serialised output.
fn audit_contains_operation(lines: &[String], operation: &str, decision: &str) -> bool {
    lines.iter().any(|line| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            v.get("operation").and_then(|o| o.as_str()) == Some(operation)
                && v.get("decision").and_then(|d| d.as_str()) == Some(decision)
        } else {
            false
        }
    })
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn allow_policy() -> &'static str {
    "rules:\n  \
     - user: \"*\"\n    \
       org: \"test-org\"\n    \
       repo: \"test\"\n    \
       branches: [\"*\"]\n    \
       operations: [push, fetch, clone]\n    \
       effect: allow\n"
}

fn deny_policy() -> &'static str {
    "rules:\n  \
     - user: \"*\"\n    \
       org: \"test-org\"\n    \
       repo: \"test\"\n    \
       branches: [\"*\"]\n    \
       operations: [push, fetch, clone]\n    \
       effect: deny\n"
}

fn current_username() -> String {
    // The broker maps the connecting peer's UID to a passwd entry. The test
    // process runs as the same UID, so `id -un` reflects the same name the
    // broker will resolve via SO_PEERCRED.
    let out = Command::new("id")
        .arg("-un")
        .output()
        .expect("invoke id -un");
    assert!(out.status.success(), "id -un failed");
    String::from_utf8(out.stdout)
        .expect("id -un is utf-8")
        .trim()
        .to_string()
}

fn run_shim(daemon: &DaemonHandle, work_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(bin());
    cmd.arg("git");
    for a in args {
        cmd.arg(a);
    }
    cmd.env("GHBRK_SOCKET", &daemon.socket_path)
        .current_dir(work_dir);
    cmd.output().expect("spawn ghbrk git shim")
}

fn list_remote_main(private_key_path: &Path) -> Option<String> {
    // Use real git outside the broker to introspect the bare repo state. This
    // path is independent of the broker pipeline and reflects ground truth.
    let ssh_cmd = format!(
        "ssh -i {} -p {} -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no",
        private_key_path.display(),
        HOST_PORT
    );
    let out = Command::new(REAL_GIT)
        .args([
            "ls-remote",
            "ssh://git@localhost/home/git/repos/test.git",
            "refs/heads/main",
        ])
        .env("GIT_SSH_COMMAND", ssh_cmd)
        .output()
        .expect("ls-remote spawn");
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().next()?;
    let sha = line.split_whitespace().next()?;
    Some(sha.to_string())
}

fn make_commit(work_dir: &Path) {
    fs::write(work_dir.join("note.txt"), "ghbrk integration\n").unwrap();
    let runs: &[&[&str]] = &[
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "ghbrk integration"],
        &["add", "note.txt"],
        &["commit", "-m", "harness commit"],
    ];
    for args in runs {
        let out = Command::new("git")
            .args(args.iter().copied())
            .current_dir(work_dir)
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn harness_ssh_server_reachable() {
    if skip_if_no_docker("harness_ssh_server_reachable") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();

    let out = Command::new("ssh-keyscan")
        .args(["-T", "5", "-p", &HOST_PORT.to_string(), "localhost"])
        .output()
        .expect("ssh-keyscan");
    assert!(out.status.success(), "ssh-keyscan failed");
    assert!(
        !out.stdout.is_empty(),
        "ssh-keyscan returned no host keys; container not ready"
    );
}

#[test]
fn e2e_clone_succeeds() {
    if skip_if_no_docker("e2e_clone_succeeds") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();

    let user = current_username();
    let ssh = SshHarness::setup(&user);
    // Seed the bare repo with one commit so clone produces a working tree.
    seed_initial_commit(&ssh);
    let daemon = DaemonHandle::start(ssh.creds_root(), allow_policy());

    let work_dir = tempfile::tempdir().unwrap();
    let dest = work_dir.path().join("checkout");
    let out = run_shim(
        &daemon,
        work_dir.path(),
        &["clone", HARNESS_GIT_URL, dest.to_str().unwrap()],
    );

    assert!(
        out.status.success(),
        "clone failed: code={:?} stderr={} stdout={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(dest.join(".git").is_dir(), "missing .git in clone");
    assert!(dest.join("seed.txt").is_file(), "seed file not checked out");

    // Audit must have at least one allow record for the clone.
    let lines = daemon.audit_lines();
    assert!(
        audit_contains_operation(&lines, "clone", "allow"),
        "no allow audit record for clone: {lines:?}"
    );
}

#[test]
fn e2e_push_allowed() {
    if skip_if_no_docker("e2e_push_allowed") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();

    let user = current_username();
    let ssh = SshHarness::setup(&user);
    seed_initial_commit(&ssh);
    let daemon = DaemonHandle::start(ssh.creds_root(), allow_policy());

    let work_dir = tempfile::tempdir().unwrap();
    let dest = work_dir.path().join("checkout");

    let clone = run_shim(
        &daemon,
        work_dir.path(),
        &["clone", HARNESS_GIT_URL, dest.to_str().unwrap()],
    );
    assert!(
        clone.status.success(),
        "clone in push test failed: stderr={}",
        String::from_utf8_lossy(&clone.stderr)
    );

    let before = list_remote_main(&ssh.private_key_path).expect("seeded ref must exist");

    make_commit(&dest);
    let push = run_shim(&daemon, &dest, &["push", "origin", "main"]);
    assert!(
        push.status.success(),
        "push failed: code={:?} stderr={} stdout={}",
        push.status.code(),
        String::from_utf8_lossy(&push.stderr),
        String::from_utf8_lossy(&push.stdout)
    );

    let after = list_remote_main(&ssh.private_key_path).expect("ref still exists");
    assert_ne!(before, after, "refs/heads/main did not advance after push");

    let lines = daemon.audit_lines();
    assert!(
        audit_contains_operation(&lines, "push", "allow"),
        "no allow audit record for push: {lines:?}"
    );
}

#[test]
fn e2e_push_denied() {
    if skip_if_no_docker("e2e_push_denied") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();

    let user = current_username();
    let ssh = SshHarness::setup(&user);
    seed_initial_commit(&ssh);

    // The deny test needs a working clone to push from. Run that under an
    // allow daemon, then tear it down and bring up a deny daemon for the push.
    let dest = tempfile::tempdir().unwrap();
    let checkout = dest.path().join("checkout");
    {
        let allow_daemon = DaemonHandle::start(ssh.creds_root(), allow_policy());
        let clone = run_shim(
            &allow_daemon,
            dest.path(),
            &["clone", HARNESS_GIT_URL, checkout.to_str().unwrap()],
        );
        assert!(
            clone.status.success(),
            "preparatory clone failed: stderr={}",
            String::from_utf8_lossy(&clone.stderr)
        );
    }

    let before = list_remote_main(&ssh.private_key_path).expect("seeded ref must exist");
    make_commit(&checkout);

    let deny_daemon = DaemonHandle::start(ssh.creds_root(), deny_policy());
    let push = run_shim(&deny_daemon, &checkout, &["push", "origin", "main"]);
    assert!(
        !push.status.success(),
        "push under deny policy should fail: stdout={} stderr={}",
        String::from_utf8_lossy(&push.stdout),
        String::from_utf8_lossy(&push.stderr)
    );
    let stderr = String::from_utf8_lossy(&push.stderr);
    assert!(
        stderr.contains("denied") || stderr.contains("ghbrk"),
        "stderr should mention denial: {stderr}"
    );

    // Refs must be unchanged: the broker rejected the request before any
    // bytes hit the SSH transport.
    let after = list_remote_main(&ssh.private_key_path).expect("ref still exists");
    assert_eq!(
        before, after,
        "refs/heads/main advanced despite deny policy"
    );

    let lines = deny_daemon.audit_lines();
    assert!(
        audit_contains_operation(&lines, "push", "deny"),
        "no deny audit record for push: {lines:?}"
    );
}

#[test]
fn harness_teardown_clean() {
    if skip_if_no_docker("harness_teardown_clean") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let _compose = start_compose();
        // Verify the container is up first so the teardown assertion is
        // meaningful.
        let ps = Command::new("docker")
            .args(["ps", "--filter", &format!("name={CONTAINER_NAME}"), "-q"])
            .output()
            .expect("docker ps");
        assert!(
            !ps.stdout.is_empty(),
            "expected container running before teardown"
        );
    }
    // Guard dropped → compose down -v ran. Verify nothing remains.
    let ps = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name={CONTAINER_NAME}"),
            "-q",
        ])
        .output()
        .expect("docker ps");
    assert!(
        ps.stdout.is_empty(),
        "containers remain after teardown: {}",
        String::from_utf8_lossy(&ps.stdout)
    );
}

// ---------------------------------------------------------------------------
// Test helpers (continued)
// ---------------------------------------------------------------------------

/// Pushes a single seed commit into the bare repo so subsequent clones produce
/// a populated working tree. Operates without going through the broker.
fn seed_initial_commit(ssh: &SshHarness) {
    let staging = tempfile::tempdir().expect("tempdir");
    let work = staging.path().join("seed");
    fs::create_dir_all(&work).unwrap();

    let ssh_cmd = format!(
        "ssh -i {} -p {} -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no",
        ssh.private_key_path.display(),
        HOST_PORT
    );

    let runs: &[&[&str]] = &[
        &["init", "-b", "main"],
        &["config", "user.email", "seed@example.com"],
        &["config", "user.name", "seed"],
        &["commit", "--allow-empty", "-m", "init"],
    ];
    for args in runs {
        let out = Command::new(REAL_GIT)
            .args(args.iter().copied())
            .current_dir(&work)
            .output()
            .expect("git seed step");
        assert!(
            out.status.success(),
            "seed git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fs::write(work.join("seed.txt"), "seed\n").unwrap();
    for args in &[
        &["add", "seed.txt"][..],
        &["commit", "-m", "seed file"][..],
        &[
            "push",
            "ssh://git@localhost/home/git/repos/test.git",
            "main:refs/heads/main",
        ][..],
    ] {
        let out = Command::new(REAL_GIT)
            .args(args.iter().copied())
            .current_dir(&work)
            .env("GIT_SSH_COMMAND", &ssh_cmd)
            .output()
            .expect("git seed push step");
        assert!(
            out.status.success(),
            "seed git {args:?} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = ssh; // keep harness alive for clarity; key file used above.
}

// ---------------------------------------------------------------------------
// gh api → broker harness (mock GitHub over TLS)
//
// Unlike the SSH git tests, these run the *entire* broker pipeline inside the
// `devenv` container: the `mock-github` service is only reachable on the Docker
// network, and its TLS certificate is only trusted inside `devenv` (the CA is
// installed into that image's trust store at build time). The daemon and the
// `gh` child it spawns must therefore both execute inside `devenv`.
//
// The daemon and shim are the host-built `ghbrk` binary, statically linked
// against musl so it runs unchanged on the container's (older) glibc. The test
// builds that target, copies the binary into `devenv`, plants synthetic
// credentials with mode 0600, copies in an allow-all `gh_api_read` policy,
// starts the daemon inside the container with `GH_HOST=mock-github`, and runs
// `ghbrk gh api user` through it.
// ---------------------------------------------------------------------------

/// Waits until the `devenv` container responds to `docker exec ... true`.
fn wait_for_devenv(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let out = Command::new("docker")
            .args(["exec", DEVENV_CONTAINER, "true"])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("devenv container not reachable within {timeout:?}");
}

/// Builds the static musl `ghbrk` binary and returns its path. The container's
/// glibc is older than the test host's, so the dynamically linked
/// `CARGO_BIN_EXE_ghbrk` would fail to load inside `devenv`; a static musl
/// build runs unchanged. The build is incremental, so repeat runs are cheap.
fn build_static_binary() -> PathBuf {
    const TARGET: &str = "x86_64-unknown-linux-musl";
    let out = Command::new("cargo")
        .args(["build", "--release", "--target", TARGET])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("invoke cargo build for musl target");
    assert!(
        out.status.success(),
        "failed to build static musl binary: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(TARGET)
        .join("release")
        .join("ghbrk");
    assert!(bin.is_file(), "static binary missing at {}", bin.display());
    bin
}

/// Runs `docker exec <container> <args...>` and returns the captured output.
fn docker_exec(container: &str, args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .arg("exec")
        .arg(container)
        .args(args)
        .output()
        .expect("docker exec")
}

/// Writes `contents` to `path` inside `container` with the given octal mode,
/// without exposing the bytes on the command line (uses `tee` over stdin).
fn write_file_in_container(container: &str, path: &str, contents: &str, mode: &str) {
    use std::process::Stdio;
    let mut child = Command::new("docker")
        .args(["exec", "-i", container, "tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("docker exec tee");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(contents.as_bytes())
        .expect("write file contents to container");
    let status = child.wait().expect("wait tee");
    assert!(status.success(), "tee {path} failed: {status:?}");

    let chmod = docker_exec(container, &["chmod", mode, path]);
    assert!(
        chmod.status.success(),
        "chmod {mode} {path} failed: {}",
        String::from_utf8_lossy(&chmod.stderr)
    );
}

/// Allow-all `gh_api_read` policy: matches any user/org/repo. Mirrors the
/// wildcard user-scoped rule the broker resolves `gh api` requests against.
fn gh_api_allow_policy() -> &'static str {
    "rules:\n  \
     - user: \"*\"\n    \
       org: \"*\"\n    \
       repo: \"*\"\n    \
       operations: [gh_api_read]\n    \
       effect: allow\n"
}

/// Provisions the `devenv` container for a broker `gh api` run: copies the
/// static binary in, plants the policy, and creates the credential tree for
/// `root` (the container's only user). When `with_token` is false the `token`
/// file is omitted so the broker's credential load fails.
fn provision_devenv(with_token: bool) {
    let bin = build_static_binary();
    let cp = Command::new("docker")
        .args([
            "cp",
            &bin.to_string_lossy(),
            &format!("{DEVENV_CONTAINER}:{CONTAINER_BIN}"),
        ])
        .output()
        .expect("docker cp ghbrk binary");
    assert!(
        cp.status.success(),
        "docker cp binary failed: {}",
        String::from_utf8_lossy(&cp.stderr)
    );
    let chmod = docker_exec(DEVENV_CONTAINER, &["chmod", "755", CONTAINER_BIN]);
    assert!(chmod.status.success(), "chmod binary failed");

    // The daemon resolves credentials for the connecting peer's username. The
    // shim runs as root inside devenv, so the creds dir must be keyed "root".
    let creds_user_dir = "/tmp/ghbrk-creds/root";
    let mk = docker_exec(DEVENV_CONTAINER, &["mkdir", "-p", creds_user_dir]);
    assert!(mk.status.success(), "mkdir creds dir failed");

    // The broker requires the SSH key file to exist with mode 0600 even for a
    // `gh api` call (credential loading is uniform across tools).
    write_file_in_container(
        DEVENV_CONTAINER,
        &format!("{creds_user_dir}/id_rsa"),
        "placeholder-key\n",
        "600",
    );
    if with_token {
        write_file_in_container(
            DEVENV_CONTAINER,
            &format!("{creds_user_dir}/token"),
            FAKE_TOKEN,
            "600",
        );
    }

    write_file_in_container(
        DEVENV_CONTAINER,
        "/tmp/ghbrk-policy.yaml",
        gh_api_allow_policy(),
        "644",
    );
}

/// Starts the daemon inside `devenv` (detached) with `GH_HOST=mock-github` and
/// waits for its socket to appear. Returns once the socket is a live socket
/// file or panics on timeout.
fn start_daemon_in_devenv() {
    // Clear any stale socket from a previous run within the same container.
    let _ = docker_exec(DEVENV_CONTAINER, &["rm", "-f", CONTAINER_SOCKET]);

    let detached = Command::new("docker")
        .args([
            "exec",
            "-d",
            DEVENV_CONTAINER,
            "env",
            &format!("GHBRK_SOCKET={CONTAINER_SOCKET}"),
            "GHBRK_POLICY=/tmp/ghbrk-policy.yaml",
            "GHBRK_CREDENTIALS_ROOT=/tmp/ghbrk-creds",
            "GHBRK_AUDIT_LOG=/tmp/ghbrk-audit.log",
            &format!("GH_HOST={MOCK_GH_HOST}"),
            CONTAINER_BIN,
            "daemon",
        ])
        .output()
        .expect("docker exec -d daemon");
    assert!(
        detached.status.success(),
        "failed to start daemon in devenv: {}",
        String::from_utf8_lossy(&detached.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let test = docker_exec(DEVENV_CONTAINER, &["test", "-S", CONTAINER_SOCKET]);
        if test.status.success() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("daemon socket {CONTAINER_SOCKET} did not appear in devenv within 10s");
}

/// Best-effort kill of the in-container daemon between tests.
fn stop_daemon_in_devenv() {
    let _ = docker_exec(DEVENV_CONTAINER, &["pkill", "-f", "ghbrk daemon"]);
    let _ = docker_exec(DEVENV_CONTAINER, &["rm", "-f", CONTAINER_SOCKET]);
}

/// Runs `ghbrk gh api user` through the in-container daemon and returns the
/// captured output.
fn run_gh_api_user_in_devenv() -> std::process::Output {
    Command::new("docker")
        .args([
            "exec",
            DEVENV_CONTAINER,
            "env",
            &format!("GHBRK_SOCKET={CONTAINER_SOCKET}"),
            CONTAINER_BIN,
            "gh",
            "api",
            "user",
        ])
        .output()
        .expect("docker exec gh api user")
}

#[test]
fn devenv_container_reachable_as_root() {
    if skip_if_no_docker("devenv_container_reachable_as_root") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();
    wait_for_devenv(Duration::from_secs(120));

    let id = docker_exec(DEVENV_CONTAINER, &["id"]);
    assert!(id.status.success(), "id failed in devenv");
    let id_out = String::from_utf8_lossy(&id.stdout);
    assert!(
        id_out.contains("uid=0(root)"),
        "devenv must run as root: {id_out}"
    );

    let gh = docker_exec(DEVENV_CONTAINER, &["gh", "--version"]);
    assert!(
        gh.status.success(),
        "gh --version failed in devenv: {}",
        String::from_utf8_lossy(&gh.stderr)
    );

    let git = docker_exec(DEVENV_CONTAINER, &["git", "--version"]);
    assert!(
        git.status.success(),
        "git --version failed in devenv: {}",
        String::from_utf8_lossy(&git.stderr)
    );
}

#[test]
fn mock_github_reachable_over_tls() {
    if skip_if_no_docker("mock_github_reachable_over_tls") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();
    wait_for_devenv(Duration::from_secs(120));

    // Curl over TLS *without* -k: success proves the test CA is trusted and the
    // server certificate's SAN matches `mock-github`.
    let out = docker_exec(
        DEVENV_CONTAINER,
        &[
            "curl",
            "-s",
            "-w",
            "\n%{http_code}",
            "https://mock-github/api/v3/user",
            "-H",
            &format!("Authorization: bearer {FAKE_TOKEN}"),
        ],
    );
    assert!(
        out.status.success(),
        "curl to mock-github failed (TLS trust?): stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    let code = body.lines().last().unwrap_or("");
    assert_eq!(code, "200", "expected HTTP 200, got body:\n{body}");
    assert!(
        body.contains("\"login\""),
        "mock response missing login field: {body}"
    );
}

#[test]
fn gh_api_through_broker_succeeds() {
    if skip_if_no_docker("gh_api_through_broker_succeeds") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();
    wait_for_devenv(Duration::from_secs(120));

    provision_devenv(true);
    start_daemon_in_devenv();
    let out = run_gh_api_user_in_devenv();
    stop_daemon_in_devenv();

    assert!(
        out.status.success(),
        "gh api user through broker failed: code={:?} stdout={} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"login\""),
        "broker gh api response missing login: stdout={stdout}"
    );
}

#[test]
fn gh_api_broker_missing_token() {
    if skip_if_no_docker("gh_api_broker_missing_token") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();
    wait_for_devenv(Duration::from_secs(120));

    provision_devenv(false);
    start_daemon_in_devenv();
    let out = run_gh_api_user_in_devenv();
    stop_daemon_in_devenv();

    assert!(
        !out.status.success(),
        "gh api user must fail without a token: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("credential") || combined.contains("token") || combined.contains("ghbrk"),
        "expected a meaningful credential error: {combined}"
    );
}

// ---------------------------------------------------------------------------
// Privilege-drop e2e (Group F, task 6.2)
//
// Proves `ghbrk git push` succeeds for a user whose home directory is mode
// `0700` *without* `chmod o+x ~`. The whole pipeline runs inside `devenv`:
//
//   * The daemon runs as root and is the only process that can read the
//     0700 home dir before privilege drop.
//   * `priv-testuser` (uid 2001, see Dockerfile.devenv) connects the shim.
//   * The daemon resolves the peer via SO_PEERCRED, then drops privileges to
//     uid 2001 before spawning git, so git traverses `/home/priv-testuser`
//     (mode 0700) as the owner rather than as root.
//
// If privilege drop were broken — i.e. git ran as root through a 0700 home —
// SSH or git would refuse, and the push assertion below would fail.
//
// Unlike the SSH git tests above (which drive a host-built daemon against the
// host-published port 2222), this test reaches `git-server` over the Docker
// network at hostname `git-server` port 22. The URL-rewrite wrapper therefore
// targets `ssh://git@git-server/home/git/repos/` and omits the host port.
// ---------------------------------------------------------------------------

/// Credentials root, policy, audit log, socket, and wrapper dir used by the
/// privilege-drop test. Kept separate from the `gh api` test's paths so the
/// two suites never collide on a shared file.
const PRIV_CREDS_ROOT: &str = "/tmp/priv-drop-creds";
const PRIV_POLICY: &str = "/tmp/priv-drop-policy.yaml";
const PRIV_AUDIT: &str = "/tmp/priv-drop-audit.log";
const PRIV_SOCKET: &str = "/tmp/priv-drop.sock";
const PRIV_BIN_DIR: &str = "/tmp/priv-drop-bin";
const PRIV_KEY: &str = "/tmp/priv-drop-id_rsa";
const PRIV_HOME: &str = "/home/priv-testuser";

/// `ls-remote` for `refs/heads/main` of the bare repo, executed from inside
/// `devenv` over the Docker network using the test's SSH key. Returns the SHA
/// of `refs/heads/main` if it exists. Used both to wait for the server and to
/// snapshot the ref before/after the push.
fn priv_remote_main_sha() -> Option<String> {
    let ssh =
        format!("ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {PRIV_KEY}");
    let out = Command::new("docker")
        .args([
            "exec",
            DEVENV_CONTAINER,
            "env",
            &format!("GIT_SSH_COMMAND={ssh}"),
            "/usr/bin/git",
            "ls-remote",
            "ssh://git@git-server/home/git/repos/test.git",
            "refs/heads/main",
        ])
        .output()
        .expect("ls-remote in devenv");
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .map(str::to_string)
}

#[test]
fn e2e_privilege_drop_0700_home() {
    if skip_if_no_docker("e2e_privilege_drop_0700_home") {
        return;
    }
    let _lock = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _compose = start_compose();
    wait_for_devenv(Duration::from_secs(120));

    // Copy the static musl binary into devenv (same pattern as provision_devenv).
    let bin_path = build_static_binary();
    let cp = Command::new("docker")
        .args([
            "cp",
            bin_path.to_str().unwrap(),
            &format!("{DEVENV_CONTAINER}:{CONTAINER_BIN}"),
        ])
        .output()
        .expect("docker cp");
    assert!(
        cp.status.success(),
        "docker cp failed: {}",
        String::from_utf8_lossy(&cp.stderr)
    );
    let chmod = docker_exec(DEVENV_CONTAINER, &["chmod", "755", CONTAINER_BIN]);
    assert!(chmod.status.success(), "chmod binary failed");

    // Generate an ed25519 keypair inside devenv (as root). Generating in the
    // container avoids any host keygen dependency differences.
    docker_exec(
        DEVENV_CONTAINER,
        &["rm", "-f", PRIV_KEY, &format!("{PRIV_KEY}.pub")],
    );
    let kg = docker_exec(
        DEVENV_CONTAINER,
        &[
            "ssh-keygen",
            "-t",
            "ed25519",
            "-N",
            "",
            "-q",
            "-C",
            "priv-drop-test",
            "-f",
            PRIV_KEY,
        ],
    );
    assert!(
        kg.status.success(),
        "ssh-keygen in devenv failed: {}",
        String::from_utf8_lossy(&kg.stderr)
    );
    let pubkey_out = docker_exec(DEVENV_CONTAINER, &["cat", &format!("{PRIV_KEY}.pub")]);
    assert!(pubkey_out.status.success(), "read pubkey failed");
    let public_key = String::from_utf8_lossy(&pubkey_out.stdout)
        .trim()
        .to_string();

    // Authorize the key on git-server (reachable from the host at HOST_PORT,
    // which is the same container the Docker network exposes as `git-server`).
    inject_authorized_key(&public_key);

    // Wait until git-server answers a git request over the Docker network. The
    // login shell is git-shell, so only a real git op (ls-remote) is a valid
    // reachability probe. An empty bare repo returns success with no refs.
    let net_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ssh = format!(
            "ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=3 -i {PRIV_KEY}"
        );
        let probe = Command::new("docker")
            .args([
                "exec",
                DEVENV_CONTAINER,
                "env",
                &format!("GIT_SSH_COMMAND={ssh}"),
                "/usr/bin/git",
                "ls-remote",
                "ssh://git@git-server/home/git/repos/test.git",
            ])
            .output()
            .expect("ls-remote reachability probe");
        if probe.status.success() {
            break;
        }
        assert!(
            Instant::now() < net_deadline,
            "git-server not reachable from devenv: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        std::thread::sleep(Duration::from_millis(300));
    }

    // Seed the bare repo with an initial commit on `main`, entirely inside
    // devenv using the generated key. git-shell permits only git operations,
    // so seeding is done with a `git push` rather than shell commands on the
    // server. This creates refs/heads/main for the later clone/push.
    let seed = Command::new("docker")
        .args([
            "exec",
            DEVENV_CONTAINER,
            "env",
            &format!(
                "GIT_SSH_COMMAND=ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {PRIV_KEY}"
            ),
            "sh",
            "-c",
            "rm -rf /tmp/priv-drop-seed && \
             git init -b main /tmp/priv-drop-seed && \
             cd /tmp/priv-drop-seed && \
             git config user.email seed@test.local && \
             git config user.name seed && \
             echo seed > seed.txt && \
             git add seed.txt && \
             git commit -m 'priv-drop seed' && \
             git push ssh://git@git-server/home/git/repos/test.git main:refs/heads/main",
        ])
        .output()
        .expect("seed push in devenv");
    assert!(
        seed.status.success(),
        "seeding bare repo failed: stdout={} stderr={}",
        String::from_utf8_lossy(&seed.stdout),
        String::from_utf8_lossy(&seed.stderr)
    );

    // Credentials for priv-testuser. The broker reads
    // <GHBRK_CREDENTIALS_ROOT>/<username>/{id_rsa,token}; both must be 0600 and
    // readable by uid 2001 (the privilege-dropped child reads the key).
    let creds_dir = format!("{PRIV_CREDS_ROOT}/priv-testuser");
    let mk = docker_exec(DEVENV_CONTAINER, &["mkdir", "-p", &creds_dir]);
    assert!(mk.status.success(), "mkdir creds dir failed");
    let cp_key = docker_exec(
        DEVENV_CONTAINER,
        &["cp", PRIV_KEY, &format!("{creds_dir}/id_rsa")],
    );
    assert!(cp_key.status.success(), "copy key into creds failed");
    docker_exec(
        DEVENV_CONTAINER,
        &["chmod", "600", &format!("{creds_dir}/id_rsa")],
    );
    // The harness never exercises HTTPS auth, so the token is a placeholder.
    write_file_in_container(
        DEVENV_CONTAINER,
        &format!("{creds_dir}/token"),
        "placeholder\n",
        "600",
    );
    // Keep the creds tree owned by priv-testuser so the pre-broker clone step
    // (which uses -i <key> directly as priv-testuser) can read it. The daemon
    // (root) also reads the key via ssh-add inside start_ssh_agent — root bypasses
    // file-permission checks, so it can read any 0600 file regardless of ownership.
    let chown = docker_exec(
        DEVENV_CONTAINER,
        &[
            "chown",
            "-R",
            "priv-testuser:priv-testuser",
            PRIV_CREDS_ROOT,
        ],
    );
    assert!(chown.status.success(), "chown creds failed");

    // Allow policy scoped to priv-testuser / test-org / test.
    write_file_in_container(
        DEVENV_CONTAINER,
        PRIV_POLICY,
        "rules:\n  \
         - user: \"priv-testuser\"\n    \
           org: \"test-org\"\n    \
           repo: \"test\"\n    \
           branches: [\"*\"]\n    \
           operations: [push, fetch, clone]\n    \
           effect: allow\n",
        "644",
    );

    // URL-rewrite wrapper on the daemon's PATH. The broker resolves git through
    // its own PATH, so the wrapper must shadow /usr/bin/git for the daemon. It
    // rewrites the canonical GitHub URL to the Docker-network git-server and
    // appends host-key options. SSH authentication now comes from SSH_AUTH_SOCK
    // (injected by the broker's ssh-agent escrow), not from GIT_SSH_COMMAND.
    docker_exec(DEVENV_CONTAINER, &["mkdir", "-p", PRIV_BIN_DIR]);
    write_file_in_container(
        DEVENV_CONTAINER,
        &format!("{PRIV_BIN_DIR}/git"),
        "#!/bin/sh\n\
         GIT_SSH_COMMAND=\"ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null\"\n\
         export GIT_SSH_COMMAND\n\
         exec /usr/bin/git \\\n  \
           -c \"url.ssh://git@git-server/home/git/repos/.insteadOf=ssh://git@github.com/test-org/\" \\\n  \
           \"$@\"\n",
        "755",
    );

    // Create ghbrk-clients group: required by start_ssh_agent to set access
    // control on the agent socket (0660 root:ghbrk-clients). Without it the
    // daemon cannot start the ssh-agent escrow.
    let mkgrp = docker_exec(DEVENV_CONTAINER, &["groupadd", "--system", "ghbrk-clients"]);
    assert!(
        mkgrp.status.success(),
        "groupadd ghbrk-clients failed: {}",
        String::from_utf8_lossy(&mkgrp.stderr)
    );
    // Add priv-testuser to ghbrk-clients (agent socket) and root (broker socket).
    let grp = docker_exec(
        DEVENV_CONTAINER,
        &["usermod", "-aG", "root,ghbrk-clients", "priv-testuser"],
    );
    assert!(
        grp.status.success(),
        "usermod add priv-testuser to groups failed: {}",
        String::from_utf8_lossy(&grp.stderr)
    );

    // Start the daemon as root, with the wrapper first on PATH so the broker's
    // git lookup resolves to it. Separate socket from the gh-api suite.
    let _ = docker_exec(DEVENV_CONTAINER, &["rm", "-f", PRIV_SOCKET]);
    let detached = Command::new("docker")
        .args([
            "exec",
            "-d",
            DEVENV_CONTAINER,
            "env",
            &format!("GHBRK_SOCKET={PRIV_SOCKET}"),
            &format!("GHBRK_POLICY={PRIV_POLICY}"),
            &format!("GHBRK_AUDIT_LOG={PRIV_AUDIT}"),
            &format!("GHBRK_CREDENTIALS_ROOT={PRIV_CREDS_ROOT}"),
            &format!("PATH={PRIV_BIN_DIR}:/usr/bin:/bin"),
            CONTAINER_BIN,
            "daemon",
        ])
        .output()
        .expect("docker exec -d daemon");
    assert!(
        detached.status.success(),
        "start daemon failed: {}",
        String::from_utf8_lossy(&detached.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let t = docker_exec(DEVENV_CONTAINER, &["test", "-S", PRIV_SOCKET]);
        if t.status.success() {
            break;
        }
        assert!(Instant::now() < deadline, "daemon socket did not appear");
        std::thread::sleep(Duration::from_millis(200));
    }

    // Clone into priv-testuser's 0700 home, as priv-testuser. The clone is run
    // directly (not through the broker) so we supply GIT_SSH_COMMAND explicitly;
    // the creds-dir copy of the key is used since the root-owned 0600 PRIV_KEY
    // is unreadable to priv-testuser.
    //
    // Use /usr/bin/git (not the PRIV_BIN_DIR wrapper) and the docker-internal
    // URL directly. The wrapper overwrites GIT_SSH_COMMAND, dropping the -i
    // flag and causing SSH to fall back to default identity lookup (which fails
    // because priv-testuser has no ~/.ssh key).
    let clone = Command::new("docker")
        .args([
            "exec",
            "--user",
            "priv-testuser",
            "-e",
            &format!("HOME={PRIV_HOME}"),
            "-e",
            &format!(
                "GIT_SSH_COMMAND=ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i {PRIV_CREDS_ROOT}/priv-testuser/id_rsa"
            ),
            DEVENV_CONTAINER,
            "/usr/bin/git",
            "clone",
            "ssh://git@git-server/home/git/repos/test.git",
            &format!("{PRIV_HOME}/testrepo"),
        ])
        .output()
        .expect("docker exec git clone");
    assert!(
        clone.status.success(),
        "clone as priv-testuser failed: stdout={} stderr={}",
        String::from_utf8_lossy(&clone.stdout),
        String::from_utf8_lossy(&clone.stderr)
    );

    // The clone was done directly against the docker-internal git-server URL, so
    // .git/config stores that URL as origin. The broker resolver only accepts
    // github.com URLs, so rewrite the remote to the canonical GitHub URL before
    // running the shim. The PRIV_BIN_DIR wrapper will translate it back via
    // url.insteadOf when git actually executes the push.
    let set_remote = Command::new("docker")
        .args([
            "exec",
            "--user",
            "priv-testuser",
            "-e",
            &format!("HOME={PRIV_HOME}"),
            DEVENV_CONTAINER,
            "/usr/bin/git",
            "-C",
            &format!("{PRIV_HOME}/testrepo"),
            "remote",
            "set-url",
            "origin",
            HARNESS_GIT_URL,
        ])
        .output()
        .expect("git remote set-url");
    assert!(
        set_remote.status.success(),
        "remote set-url failed: {}",
        String::from_utf8_lossy(&set_remote.stderr)
    );

    // Make a commit as priv-testuser.
    let commit = Command::new("docker")
        .args([
            "exec",
            "--user",
            "priv-testuser",
            "-e",
            &format!("HOME={PRIV_HOME}"),
            DEVENV_CONTAINER,
            "sh",
            "-c",
            "cd /home/priv-testuser/testrepo && \
             git config user.email 'priv@test.local' && \
             git config user.name 'priv-testuser' && \
             echo change > priv-change.txt && \
             git add priv-change.txt && \
             git commit -m 'priv-drop test commit'",
        ])
        .output()
        .expect("git commit as priv-testuser");
    assert!(
        commit.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    let before = priv_remote_main_sha().expect("seeded main ref must exist");

    // THE KEY ASSERTION: push through the ghbrk shim as priv-testuser. The
    // daemon (root) resolves the peer as uid 2001, drops privileges, and spawns
    // git as priv-testuser so it can traverse the 0700 home.
    // The shim resolves the repo from its current working directory (not a `-C`
    // flag), so run it inside the checkout via docker's --workdir.
    let push = Command::new("docker")
        .args([
            "exec",
            "--user",
            "priv-testuser",
            "--workdir",
            &format!("{PRIV_HOME}/testrepo"),
            "-e",
            &format!("HOME={PRIV_HOME}"),
            "-e",
            &format!("GHBRK_SOCKET={PRIV_SOCKET}"),
            "-e",
            &format!("PATH={PRIV_BIN_DIR}:/usr/bin:/bin"),
            DEVENV_CONTAINER,
            CONTAINER_BIN,
            "git",
            "push",
            "origin",
            "main",
        ])
        .output()
        .expect("ghbrk git push as priv-testuser");
    assert!(
        push.status.success(),
        "push through 0700 home dir failed (privilege drop not working?): \
         stdout={} stderr={}",
        String::from_utf8_lossy(&push.stdout),
        String::from_utf8_lossy(&push.stderr)
    );

    // The home dir must still be 0700 — nothing relaxed it to make the push work.
    let stat = docker_exec(DEVENV_CONTAINER, &["stat", "-c", "%a", PRIV_HOME]);
    let mode = String::from_utf8_lossy(&stat.stdout).trim().to_string();
    assert_eq!(
        mode, "700",
        "home dir mode changed! expected 700, got {mode}"
    );

    // The remote ref must have advanced.
    let after = priv_remote_main_sha();
    assert!(after.is_some(), "refs/heads/main missing after push");
    assert_ne!(
        Some(before),
        after,
        "refs/heads/main did not advance after push"
    );

    // The audit log must contain an allow record for the push.
    let audit = docker_exec(DEVENV_CONTAINER, &["cat", PRIV_AUDIT]);
    let audit_str = String::from_utf8_lossy(&audit.stdout);
    let audit_lines: Vec<String> = audit_str
        .lines()
        .filter(|l| !l.is_empty())
        .map(|s| s.to_string())
        .collect();
    assert!(
        audit_contains_operation(&audit_lines, "push", "allow"),
        "expected allow audit record for push, audit log: {audit_str}"
    );

    // Best-effort daemon teardown; the compose guard removes the project on drop.
    let _ = docker_exec(DEVENV_CONTAINER, &["pkill", "-f", "ghbrk daemon"]);
}
