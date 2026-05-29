//! `ghbrk check` — verifies that the current user's credentials exist and
//! have correct permissions, then optionally pings the GitHub API.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ghbrk::credentials::{credential_paths_in, credentials_dir};

const REQUIRED_MODE: u32 = 0o600;
const PERMISSION_MASK: u32 = 0o777;

/// Runs the `check` subcommand: verifies credentials and optionally pings the
/// GitHub API.
pub fn run() -> ExitCode {
    let base = credentials_root();
    let user = current_user();

    let paths = match credential_paths_in(&base, &user) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ghbrk check: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut all_ok = true;

    all_ok &= check_file("SSH key", &paths.ssh_key);
    all_ok &= check_file("Token", &paths.token);
    check_github_api(&paths.token, &mut all_ok);

    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Returns the credentials root directory, honouring `GHBRK_CREDENTIALS_ROOT`.
fn credentials_root() -> PathBuf {
    std::env::var_os("GHBRK_CREDENTIALS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(credentials_dir)
}

/// Returns the current Unix username.  Tries `USER` then `LOGNAME` env vars,
/// then falls back to the numeric uid.
fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| format!("uid{}", nix::unistd::getuid().as_raw()))
}

/// Checks that `path` exists and has exactly mode 0600.  Prints one status
/// line and returns `true` iff the check passes.
fn check_file(label: &str, path: &Path) -> bool {
    match fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("{label}: MISSING");
            false
        }
        Err(e) => {
            println!("{label}: ERROR ({e})");
            false
        }
        Ok(meta) => {
            let mode = meta.permissions().mode() & PERMISSION_MASK;
            if mode == REQUIRED_MODE {
                println!("{label}: OK");
                true
            } else {
                println!("{label}: BAD PERMISSIONS ({mode:#05o})");
                false
            }
        }
    }
}

/// Pings the GitHub user endpoint using the token read from `token_path`.
/// Skips gracefully (prints a notice, does not fail) when
/// `GH_TOKEN` env var is absent and the token file cannot be read.
fn check_github_api(token_path: &Path, ok: &mut bool) {
    let token = match read_token_if_available(token_path) {
        Some(t) => t,
        None => {
            println!("GitHub API: SKIPPED (no token available)");
            return;
        }
    };

    match ping_github(&token) {
        GithubResult::Ok(login) => {
            println!("GitHub API: OK (user: {login})");
        }
        GithubResult::InvalidToken => {
            println!("GitHub API: INVALID TOKEN");
            *ok = false;
        }
        GithubResult::Unreachable => {
            println!("GitHub API: UNREACHABLE");
            *ok = false;
        }
    }
}

/// Reads the token from `path` if it exists and is readable.  Returns `None`
/// when the file is absent.  Does not check permissions — that is already
/// reported by `check_file`.
fn read_token_if_available(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(raw) => Some(raw.trim_end_matches(['\n', '\r']).to_string()),
        Err(_) => None,
    }
}

enum GithubResult {
    Ok(String),
    InvalidToken,
    Unreachable,
}

/// Returns the GitHub API user endpoint, honouring `GHBRK_GITHUB_API_URL` for
/// testability. Production leaves the variable unset and uses the default.
fn github_api_url() -> String {
    std::env::var("GHBRK_GITHUB_API_URL")
        .unwrap_or_else(|_| "https://api.github.com/user".to_string())
}

/// Performs `GET` against the GitHub user endpoint and classifies the outcome.
fn ping_github(token: &str) -> GithubResult {
    use ureq::Error;

    let result = ureq::get(github_api_url())
        .header("Authorization", &format!("Bearer {token}"))
        .header("User-Agent", "ghbrk/check")
        .header("Accept", "application/vnd.github+json")
        .config()
        .http_status_as_error(false)
        .build()
        .call();

    match result {
        Ok(mut resp) => {
            let status = resp.status();
            if status == 200 {
                let login = resp
                    .body_mut()
                    .read_to_string()
                    .ok()
                    .and_then(|body| parse_json_string_field(&body, "login"))
                    .unwrap_or_else(|| "<unknown>".to_string());
                GithubResult::Ok(login)
            } else if status == 401 {
                GithubResult::InvalidToken
            } else {
                GithubResult::Unreachable
            }
        }
        Err(Error::Io(_) | Error::HostNotFound | Error::Timeout(_)) => GithubResult::Unreachable,
        Err(_) => GithubResult::Unreachable,
    }
}

/// Extracts a top-level string field from a JSON object without pulling in a
/// JSON crate.  Returns `None` if the field is absent or malformed.
fn parse_json_string_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after_key = &json[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let inner = &after_colon[1..];
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}
