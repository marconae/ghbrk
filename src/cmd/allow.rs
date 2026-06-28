use ghbrk::protocol::Tool;

use super::gateway::{run_gateway, socket_path_from_env};

/// Validate that `repo` is in the `org/repo` format required by the allow command.
///
/// Returns an error string when the format is invalid.
pub fn validate_repo_spec(repo: &str) -> Result<(), String> {
    fn is_valid_segment(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    }

    match repo.split_once('/') {
        Some((org, name))
            if !name.contains('/') && is_valid_segment(org) && is_valid_segment(name) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "invalid repository specifier {repo:?}: expected org/repo format (e.g. acme/web)"
        )),
    }
}

/// Assemble the args vector to send to the broker for an allow request.
pub fn assemble_args(repo: &str, operands: &[String], user: Option<&str>) -> Vec<String> {
    let mut args = Vec::with_capacity(operands.len() + 3);
    args.push(repo.to_owned());
    args.extend_from_slice(operands);
    if let Some(u) = user {
        args.push("--user".to_owned());
        args.push(u.to_owned());
    }
    args
}

/// Entry point for `ghbrk allow`. Validates the repo spec, assembles the args,
/// and relays the request to the broker via the gateway. Never returns.
pub fn run(repo: String, operands: Vec<String>, user: Option<String>) -> ! {
    if let Err(err) = validate_repo_spec(&repo) {
        eprintln!("ghbrk: {err}");
        std::process::exit(2);
    }
    let args = assemble_args(&repo, &operands, user.as_deref());
    let cwd = std::env::current_dir().unwrap_or_default();
    run_gateway(Tool::Allow, args, cwd, &socket_path_from_env(), None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- repo spec validation ---

    #[test]
    fn rejects_malformed_repo_spec() {
        assert!(validate_repo_spec("not-a-repo-spec").is_err());
        assert!(validate_repo_spec("").is_err());
        assert!(validate_repo_spec("org/").is_err());
        assert!(validate_repo_spec("/repo").is_err());
        assert!(validate_repo_spec("org/repo/extra").is_err());
        assert!(validate_repo_spec("org repo").is_err());
    }

    #[test]
    fn accepts_valid_repo_spec() {
        assert!(validate_repo_spec("acme/web").is_ok());
        assert!(validate_repo_spec("my-org/my-repo").is_ok());
        assert!(validate_repo_spec("Org123/repo.name").is_ok());
        assert!(validate_repo_spec("a/b").is_ok());
    }

    // --- arg assembly ---

    #[test]
    fn assembles_args_without_user() {
        let operands = vec!["write".to_owned()];
        let args = assemble_args("acme/web", &operands, None);
        assert_eq!(args, vec!["acme/web", "write"]);
        assert!(!args.contains(&"--user".to_owned()));
    }

    #[test]
    fn assembles_args_with_user() {
        let operands = vec!["write".to_owned()];
        let args = assemble_args("acme/web", &operands, Some("alice"));
        assert_eq!(args, vec!["acme/web", "write", "--user", "alice"]);
    }

    #[test]
    fn assembles_args_with_multiple_operands() {
        let operands = vec!["push".to_owned(), "pr_open".to_owned()];
        let args = assemble_args("org/repo", &operands, Some("bob"));
        assert_eq!(args, vec!["org/repo", "push", "pr_open", "--user", "bob"]);
    }
}
