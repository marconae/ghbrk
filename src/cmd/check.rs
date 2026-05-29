//! `ghbrk check` — routes credential health checks through the broker so that
//! credential files (owned by `ghbrk`, mode 0600) are inspected as the broker
//! user rather than the unprivileged caller.

use ghbrk::protocol::Tool;

use super::shim::run_shim_blocking;

pub fn run() -> ! {
    run_shim_blocking(Tool::Check, &[])
}
