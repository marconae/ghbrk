use ghbrk::protocol::Tool;
use ghbrk::resolver::{find_git_dir, read_head_branch, read_origin_url};

use super::gateway::{run_gateway, socket_path_from_env};

pub fn run(args: &[String]) -> ! {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (remote_url, head_branch) = resolve_hints(&cwd);
    run_gateway(
        Tool::Gh,
        args.to_vec(),
        cwd,
        &socket_path_from_env(),
        remote_url,
        head_branch,
    )
}

fn resolve_hints(cwd: &std::path::Path) -> (Option<String>, Option<String>) {
    let git_dir = find_git_dir(cwd);
    let remote_url = git_dir
        .as_deref()
        .and_then(|gd| read_origin_url(gd).ok());
    let head_branch = git_dir.as_deref().and_then(read_head_branch);
    (remote_url, head_branch)
}
