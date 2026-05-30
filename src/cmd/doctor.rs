use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tokio::net::UnixStream;

use ghbrk::policy::Policy;
use ghbrk::protocol::{read_frame, write_frame, Request, ServerFrame, Tool};

use super::gateway::socket_path_from_env;

/// Default policy file path, overridable via `GHBRK_POLICY`.
const DEFAULT_POLICY_PATH: &str = "/etc/ghbrk/policy.yaml";

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

    let (daemon_ok, creds_ok) = runtime.block_on(check_daemon_and_creds(&socket_path));
    let policy_ok = check_policy(&policy_path);

    if daemon_ok && creds_ok && policy_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn policy_path_from_env() -> PathBuf {
    env::var_os(POLICY_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_POLICY_PATH))
}

/// Attempt to connect to the broker socket and run the Check tool.
/// Returns `(daemon_ok, creds_ok)`.
async fn check_daemon_and_creds(socket_path: &Path) -> (bool, bool) {
    match UnixStream::connect(socket_path).await {
        Err(err) => {
            println!("Daemon: UNREACHABLE ({}: {})", socket_path.display(), err);
            println!("Credentials: SKIPPED (daemon unreachable)");
            (false, false)
        }
        Ok(stream) => {
            println!("Daemon: OK");
            let creds_ok = run_check_via_broker(stream).await;
            (true, creds_ok)
        }
    }
}

/// Send a `Tool::Check` request via an already-connected stream and interpret
/// the broker's exit code plus any output it produces.
async fn run_check_via_broker(stream: UnixStream) -> bool {
    let cwd = env::current_dir().unwrap_or_default();
    let request = Request {
        tool: Tool::Check,
        args: vec![],
        cwd,
        remote_url: None,
        head_branch: None,
    };

    let (read_half, mut write_half) = stream.into_split();
    if write_frame(&mut write_half, &request).await.is_err() {
        println!("Credentials: FAILED");
        return false;
    }

    let mut reader = read_half;
    let mut broker_output = Vec::<u8>::new();
    let exit_code = loop {
        match read_frame::<_, ServerFrame>(&mut reader).await {
            Ok(ServerFrame::StdoutChunk { data }) => broker_output.extend_from_slice(&data),
            Ok(ServerFrame::StderrChunk { data }) => broker_output.extend_from_slice(&data),
            Ok(ServerFrame::Exit { code }) => break code,
            Ok(ServerFrame::Denied { reason }) => {
                println!("Credentials: FAILED");
                println!("{reason}");
                return false;
            }
            Err(_) => {
                println!("Credentials: FAILED");
                return false;
            }
        }
    };

    if exit_code == 0 {
        println!("Credentials: OK");
        true
    } else {
        println!("Credentials: FAILED");
        if !broker_output.is_empty() {
            let _ = io::Write::write_all(&mut io::stdout(), &broker_output);
        }
        false
    }
}

/// Read and parse the policy file. Prints one status line and returns success.
fn check_policy(path: &Path) -> bool {
    match std::fs::File::open(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            println!("Policy: MISSING ({})", path.display());
            false
        }
        Err(err) => {
            println!("Policy: MISSING ({}: {})", path.display(), err);
            false
        }
        Ok(file) => match Policy::from_reader(file) {
            Ok(_) => {
                println!("Policy: OK");
                true
            }
            Err(err) => {
                println!("Policy: INVALID ({})", err);
                false
            }
        },
    }
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
        let ok = check_policy(&path);
        assert!(!ok);
    }

    #[test]
    fn policy_valid_file_reports_ok() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "rules:\n  - user: \"*\"\n    org: acme\n    repo: \"*\"\n    operations: [push]\n    effect: allow"
        )
        .unwrap();
        let ok = check_policy(f.path());
        assert!(ok);
    }

    #[test]
    fn policy_invalid_yaml_reports_invalid() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "not: valid: policy: content: !!").unwrap();
        let ok = check_policy(f.path());
        assert!(!ok);
    }

    #[tokio::test]
    async fn daemon_unreachable_reports_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("absent.sock");
        let (daemon_ok, creds_ok) = check_daemon_and_creds(&socket).await;
        assert!(!daemon_ok);
        assert!(!creds_ok);
    }
}
