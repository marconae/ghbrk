use ghbrk::protocol::Tool;

use super::shim::run_shim_blocking;

/// Git shim entry point. Connects to the broker socket and relays I/O.
/// This function does not return; it exits the process with the broker's
/// reported exit code (or `1` on connect / protocol failure).
pub fn run(args: &[String]) -> ! {
    run_shim_blocking(Tool::Git, args)
}
