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
const CONTAINER_NAME: &str = "ghbrk-it-git-server";
const REMOTE_ORG: &str = "test-org";
const HARNESS_GIT_URL: &str = "ssh://git@github.com/test-org/test.git";

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
    wait_for_ssh(HOST_PORT, Duration::from_secs(30));
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
    let out = Command::new("git")
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
    let _lock = GLOBAL_LOCK.lock().unwrap();
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
    let _lock = GLOBAL_LOCK.lock().unwrap();
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
    let _lock = GLOBAL_LOCK.lock().unwrap();
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
    let _lock = GLOBAL_LOCK.lock().unwrap();
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
    let _lock = GLOBAL_LOCK.lock().unwrap();
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
        let out = Command::new("git")
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
        let out = Command::new("git")
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
