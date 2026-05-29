use std::path::Path;
use std::process;

use ghbrk::protocol::Tool;
use ghbrk::shim::{run_shim, socket_path_from_env, SHIM_ERROR_EXIT};

/// Block on the async shim core, then terminate the process with the resulting
/// exit code. This is the binary-side wrapper around `ghbrk::shim::run_shim`.
pub fn run_shim_blocking(tool: Tool, args: &[String]) -> ! {
    let socket_path = socket_path_from_env();
    let cwd = std::env::current_dir().unwrap_or_default();
    let owned_args: Vec<String> = args.to_vec();

    // Pre-resolve git context while running as the invoking user (the broker
    // runs as a different system user and may lack read access to the repo).
    let (remote_url, head_branch) = resolve_hints(&cwd);

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
        // `check` has no real binary to fall back to. If the broker socket is
        // inaccessible (EACCES), the shim core exec's this path; a nonexistent
        // path fails the exec and exits nonzero, which is the right signal that
        // the caller is not a member of the broker's client group.
        Tool::Check => "/nonexistent/ghbrk-check".to_string(),
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("ghbrk: failed to start async runtime: {err}");
            process::exit(SHIM_ERROR_EXIT);
        }
    };

    let code = runtime.block_on(async move {
        run_shim(
            tool,
            owned_args,
            cwd,
            &socket_path,
            &real_path,
            remote_url,
            head_branch,
        )
        .await
    });
    process::exit(code);
}

/// Resolve git remote URL and HEAD branch from the working directory.
///
/// Reads `.git/config` and `.git/HEAD` while running as the invoking user,
/// before the request is handed to the broker (which may run as a different
/// system user without repo read access).
pub(crate) fn resolve_hints(cwd: &Path) -> (Option<String>, Option<String>) {
    let git_dir = ghbrk::resolver::find_git_dir(cwd);
    let remote_url = git_dir
        .as_deref()
        .and_then(|gd| ghbrk::resolver::read_origin_url(gd).ok());
    let head_branch = git_dir
        .as_deref()
        .and_then(ghbrk::resolver::read_head_branch);
    (remote_url, head_branch)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use tempfile::TempDir;

    use super::resolve_hints;

    fn make_fake_repo(dir: &TempDir, origin_url: &str, head_branch: &str) {
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(
            git_dir.join("config"),
            format!(
                "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = {origin_url}\n"
            ),
        )
        .unwrap();
        fs::write(
            git_dir.join("HEAD"),
            format!("ref: refs/heads/{head_branch}\n"),
        )
        .unwrap();
    }

    #[test]
    fn hints_populated_from_git_repo() {
        let dir = TempDir::new().unwrap();
        make_fake_repo(&dir, "git@github.com:test/repo.git", "feat");

        let (remote_url, head_branch) = resolve_hints(dir.path());

        assert_eq!(remote_url, Some("git@github.com:test/repo.git".to_string()));
        assert_eq!(head_branch, Some("feat".to_string()));
    }

    #[test]
    fn hints_absent_outside_repo() {
        let dir = TempDir::new().unwrap();

        let (remote_url, head_branch) = resolve_hints(dir.path());

        assert_eq!(remote_url, None);
        assert_eq!(head_branch, None);
    }
}
