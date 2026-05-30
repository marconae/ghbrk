use ghbrk::protocol::Tool;

use super::gateway::{run_gateway, socket_path_from_env};

pub fn run(args: &[String]) -> ! {
    let cwd = std::env::current_dir().unwrap_or_default();
    run_gateway(
        Tool::Gh,
        args.to_vec(),
        cwd,
        &socket_path_from_env(),
        None,
        None,
    )
}
