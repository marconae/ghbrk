use std::process;

use ghbrk::protocol::Tool;
use ghbrk::shim::{run_shim, socket_path_from_env, SHIM_ERROR_EXIT};

/// Block on the async shim core, then terminate the process with the resulting
/// exit code. This is the binary-side wrapper around `ghbrk::shim::run_shim`.
pub fn run_shim_blocking(tool: Tool, args: &[String]) -> ! {
    let socket_path = socket_path_from_env();
    let cwd = std::env::current_dir().unwrap_or_default();
    let owned_args: Vec<String> = args.to_vec();

    let cfg = match ghbrk::config::load() {
        Ok(c) => c,
        Err((path, err)) => {
            eprintln!("ghbrk: failed to load config from {path}: {err}");
            process::exit(SHIM_ERROR_EXIT);
        }
    };
    let real_path = match tool {
        Tool::Git => cfg.real_git,
        Tool::Gh => cfg.real_gh,
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("ghbrk: failed to start async runtime: {err}");
            process::exit(SHIM_ERROR_EXIT);
        }
    };

    let code = runtime
        .block_on(async move { run_shim(tool, owned_args, cwd, &socket_path, &real_path).await });
    process::exit(code);
}
