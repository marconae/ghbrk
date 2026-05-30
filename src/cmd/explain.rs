//! `ghbrk explain <command>` — a dry-run inspector for the privilege boundary.
//!
//! Relays the command tokens to the broker as a `Tool::Explain` request and
//! streams the broker's structured explanation to stdio. The broker performs
//! all classification, resolution, and policy evaluation; this client never
//! executes git or gh and never leaves the machine.

use std::process::ExitCode;

use ghbrk::protocol::Tool;
use ghbrk::resolver::{find_git_dir, read_head_branch, read_origin_url};

use super::gateway::{relay, socket_path_from_env};

/// Run `ghbrk explain` over the supplied command tokens.
pub fn run(args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("ghbrk explain: no command provided");
        return ExitCode::FAILURE;
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let git_dir = find_git_dir(&cwd);
    let remote_url = git_dir.as_deref().and_then(|d| read_origin_url(d).ok());
    let head_branch = git_dir.as_deref().and_then(read_head_branch);
    let socket_path = socket_path_from_env();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("ghbrk explain: failed to start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    let code = runtime.block_on(async {
        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();
        relay(
            Tool::Explain,
            args.to_vec(),
            cwd,
            &socket_path,
            remote_url,
            head_branch,
            &mut stdout,
            &mut stderr,
        )
        .await
    });

    ExitCode::from(code as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explain_empty_args_fails() {
        let code = run(&[]);
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::FAILURE));
    }
}
