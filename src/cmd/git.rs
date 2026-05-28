use ghbrk::config::load as load_config;
use ghbrk::passthrough::{exec_passthrough, is_passthrough};
use ghbrk::protocol::Tool;

use super::shim::run_shim_blocking;

pub fn run(args: &[String]) -> ! {
    let cfg = load_config().unwrap_or_else(|(path, err)| {
        eprintln!("ghbrk: failed to load config {path}: {err}");
        std::process::exit(1);
    });
    if is_passthrough(Tool::Git, args) {
        exec_passthrough(&cfg.real_git, args);
    }
    run_shim_blocking(Tool::Git, args)
}
