//! Credential health checks shared by `ghbrk check` and the broker.
//!
//! These run inside the broker process (as the `ghbrk` user) so that
//! credential files owned by `ghbrk` can actually be read. Output is written
//! to a caller-supplied writer rather than stdout so the broker can stream it
//! back to the invoking client.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const REQUIRED_MODE: u32 = 0o600;
const PERMISSION_MASK: u32 = 0o777;

/// Outcome of pinging the GitHub user endpoint.
pub enum GithubResult {
    Ok(String),
    InvalidToken,
    Unreachable,
}

/// Runs all credential health checks for `user` rooted at `creds_root`,
/// writing one status line per check to `out`. Returns `true` iff every check
/// passed.
pub fn run_checks(creds_root: &Path, user: &str, out: &mut impl Write) -> bool {
    let paths = match crate::credentials::credential_paths_in(creds_root, user) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(out, "ghbrk check: {e}");
            return false;
        }
    };
    let mut all_ok = true;
    all_ok &= check_file("SSH key", &paths.ssh_key, out);
    all_ok &= check_file("Token", &paths.token, out);
    check_github_api(&paths.token, &mut all_ok, out);
    all_ok
}

fn check_file(label: &str, path: &Path, out: &mut impl Write) -> bool {
    match fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = writeln!(out, "{label}: MISSING");
            false
        }
        Err(e) => {
            let _ = writeln!(out, "{label}: ERROR ({e})");
            false
        }
        Ok(meta) => {
            let mode = meta.permissions().mode() & PERMISSION_MASK;
            if mode == REQUIRED_MODE {
                let _ = writeln!(out, "{label}: OK");
                true
            } else {
                let _ = writeln!(out, "{label}: BAD PERMISSIONS ({mode:#05o})");
                false
            }
        }
    }
}

fn check_github_api(token_path: &Path, ok: &mut bool, out: &mut impl Write) {
    let token = match read_token_if_available(token_path) {
        Some(t) => t,
        None => {
            let _ = writeln!(out, "GitHub API: SKIPPED (no token available)");
            return;
        }
    };

    match ping_github(&token) {
        GithubResult::Ok(login) => {
            let _ = writeln!(out, "GitHub API: OK (user: {login})");
        }
        GithubResult::InvalidToken => {
            let _ = writeln!(out, "GitHub API: INVALID TOKEN");
            *ok = false;
        }
        GithubResult::Unreachable => {
            let _ = writeln!(out, "GitHub API: UNREACHABLE");
            *ok = false;
        }
    }
}

fn read_token_if_available(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(raw) => Some(raw.trim_end_matches(['\n', '\r']).to_string()),
        Err(_) => None,
    }
}

fn github_api_url() -> String {
    std::env::var("GHBRK_GITHUB_API_URL")
        .unwrap_or_else(|_| "https://api.github.com/user".to_string())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_mode(path: &Path, contents: &str, mode: u32) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn run_checks_reports_missing_files() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        let ok = run_checks(dir.path(), "alice", &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(!ok);
        assert!(s.contains("SSH key: MISSING"), "{s}");
        assert!(s.contains("Token: MISSING"), "{s}");
    }

    #[test]
    fn run_checks_reports_ok_for_well_formed_credentials() {
        let dir = TempDir::new().unwrap();
        write_mode(&dir.path().join("alice/id_rsa"), "KEY", 0o600);
        write_mode(&dir.path().join("alice/token"), "tok", 0o600);
        std::env::set_var("GHBRK_GITHUB_API_URL", "http://127.0.0.1:1");
        let mut out = Vec::new();
        let ok = run_checks(dir.path(), "alice", &mut out);
        std::env::remove_var("GHBRK_GITHUB_API_URL");
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("SSH key: OK"), "{s}");
        assert!(s.contains("Token: OK"), "{s}");
        // GitHub API is unreachable here, so overall not ok.
        assert!(!ok);
        assert!(s.contains("GitHub API: UNREACHABLE"), "{s}");
    }

    #[test]
    fn run_checks_rejects_invalid_user() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        let ok = run_checks(dir.path(), "../etc", &mut out);
        let s = String::from_utf8(out).unwrap();
        assert!(!ok);
        assert!(s.contains("ghbrk check:"), "{s}");
    }

    #[test]
    fn parse_json_string_field_extracts_login() {
        let body = r#"{"login":"octocat","id":1}"#;
        assert_eq!(
            parse_json_string_field(body, "login"),
            Some("octocat".to_string())
        );
    }
}
