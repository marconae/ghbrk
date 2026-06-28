use ghbrk::protocol::Tool;
use ghbrk::resolver::repo_hints;

use super::gateway::{run_gateway, socket_path_from_env};

pub fn run(args: &[String]) -> ! {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (remote_url, head_branch) = repo_hints(&cwd);
    run_gateway(
        Tool::Gh,
        args.to_vec(),
        cwd,
        &socket_path_from_env(),
        remote_url,
        head_branch,
    )
}
