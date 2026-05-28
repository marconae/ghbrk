use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ghbrk")
}

const REAL_GIT: &str = "/usr/bin/git";

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

    fn write_stub_script(&self, name: &str, script: &str) -> PathBuf {
        let stub_path = self.dir.path().join(name);
        {
            let mut f = fs::File::create(&stub_path).expect("create stub");
            f.write_all(script.as_bytes()).unwrap();
            f.flush().unwrap();
            f.sync_all().unwrap();
        }
        let mut perms = fs::metadata(&stub_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&stub_path, perms).unwrap();
        stub_path
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

#[test]
fn passthrough_propagates_exit_code() {
    let env = TestEnv::new();

    let stub_path = env.write_stub_script("stub-gh", "#!/bin/sh\necho 'stub-gh output'\nexit 42\n");

    let config_path = env.write_config(REAL_GIT, stub_path.to_str().unwrap());
    let gh_sym = env.make_symlink("gh");

    let out = Command::new(&gh_sym)
        .arg("--version")
        .env("GHBRK_CONFIG", &config_path)
        .output()
        .expect("invoke gh symlink");

    assert_eq!(
        out.status.code(),
        Some(42),
        "expected exit code 42 from stub, got: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("stub-gh output"),
        "expected stub stdout, got: {stdout}"
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

#[test]
fn gh_auth_status_passes_through() {
    let env = TestEnv::new();

    let stub_path = env.write_stub_script("stub-gh", "#!/bin/sh\necho 'stub-gh output'\nexit 0\n");

    let config_path = env.write_config(REAL_GIT, stub_path.to_str().unwrap());
    let gh_sym = env.make_symlink("gh");

    let out = Command::new(&gh_sym)
        .args(["auth", "status"])
        .env("GHBRK_CONFIG", &config_path)
        .output()
        .expect("invoke gh symlink");

    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0 from stub, got: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("stub-gh output"),
        "expected stub stdout, got: {stdout}"
    );
}
