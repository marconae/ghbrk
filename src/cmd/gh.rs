use ghbrk::protocol::Tool;

use super::shim::run_shim_blocking;

pub fn run(args: &[String]) -> ! {
    run_shim_blocking(Tool::Gh, args)
}
