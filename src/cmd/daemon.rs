//! Daemon entry point. Loads the policy and audit-log paths from the
//! environment, opens the audit logger, and runs the broker accept loop until
//! a signal arrives.

use std::path::PathBuf;
use std::sync::Arc;

use ghbrk::audit::{AuditLogger, DEFAULT_AUDIT_PATH};
use ghbrk::broker::{run_broker, BrokerConfig};
use ghbrk::policy::Policy;

use super::gateway::DEFAULT_SOCKET_PATH;

/// Default policy file location.
pub const DEFAULT_POLICY_PATH: &str = "/etc/ghbrk/policy.yaml";

/// Daemon entry point. Exits the process; never returns.
pub fn run() -> ! {
    let code = run_inner();
    std::process::exit(code);
}

fn run_inner() -> i32 {
    // Initialise tracing if it has not already been set up. Best effort.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let socket_path = env_path("GHBRK_SOCKET", DEFAULT_SOCKET_PATH);
    let policy_path = env_path("GHBRK_POLICY", DEFAULT_POLICY_PATH);
    let audit_path = env_path("GHBRK_AUDIT_LOG", DEFAULT_AUDIT_PATH);
    let credentials_root = std::env::var_os("GHBRK_CREDENTIALS_ROOT").map(PathBuf::from);

    let policy = match std::fs::read_to_string(&policy_path)
        .map_err(|e| e.to_string())
        .and_then(|text| Policy::from_yaml(&text).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(err) => {
            eprintln!(
                "ghbrk: failed to load policy {}: {err}",
                policy_path.display()
            );
            return 2;
        }
    };

    let logger = match AuditLogger::new(&audit_path) {
        Ok(l) => Arc::new(l),
        Err(err) => {
            eprintln!(
                "ghbrk: failed to open audit log {}: {err}",
                audit_path.display()
            );
            return 2;
        }
    };

    let config = BrokerConfig {
        socket_path,
        policy,
        audit_logger: logger,
        credentials_root,
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(err) => {
            eprintln!("ghbrk: failed to start tokio runtime: {err}");
            return 2;
        }
    };

    match runtime.block_on(run_broker(config)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("ghbrk daemon: {err}");
            1
        }
    }
}

fn env_path(var: &str, default: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(default),
    }
}
