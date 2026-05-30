use std::env;
use std::process::ExitCode;

use ghbrk::protocol::Tool;

use super::gateway::{run_gateway, socket_path_from_env};

pub fn run(repo: &str) -> ExitCode {
    if !is_valid_repo_spec(repo) {
        eprintln!("ghbrk policy: invalid repo specifier '{repo}'; expected org/repo format");
        return ExitCode::FAILURE;
    }

    let cwd = env::current_dir().unwrap_or_default();
    let socket_path = socket_path_from_env();
    run_gateway(
        Tool::Policy,
        vec![repo.to_string()],
        cwd,
        &socket_path,
        None,
        None,
    )
}

/// Returns `true` when `repo` contains exactly one `/` with non-empty parts
/// on both sides.
fn is_valid_repo_spec(repo: &str) -> bool {
    let mut parts = repo.splitn(2, '/');
    let org = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("");
    !org.is_empty() && !name.is_empty() && !name.contains('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_rejects_malformed_specifier_no_slash() {
        assert!(!is_valid_repo_spec("notavalidspec"));
    }

    #[test]
    fn policy_rejects_empty_org() {
        assert!(!is_valid_repo_spec("/repo"));
    }

    #[test]
    fn policy_rejects_empty_repo() {
        assert!(!is_valid_repo_spec("org/"));
    }

    #[test]
    fn policy_accepts_valid_org_repo() {
        assert!(is_valid_repo_spec("acme/web"));
    }

    #[test]
    fn policy_accepts_valid_with_hyphens() {
        assert!(is_valid_repo_spec("my-org/my-repo"));
    }

    #[test]
    fn policy_rejects_too_many_slashes() {
        // "acme/web/extra" — the part after the first slash contains another slash
        assert!(!is_valid_repo_spec("acme/web/extra"));
    }
}
