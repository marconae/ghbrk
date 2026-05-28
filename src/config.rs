use serde::Deserialize;
use std::path::Path;

fn default_real_git() -> String {
    "/usr/bin/git".to_string()
}

fn default_real_gh() -> String {
    "/usr/bin/gh".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ShimConfig {
    #[serde(default = "default_real_git")]
    pub real_git: String,
    #[serde(default = "default_real_gh")]
    pub real_gh: String,
}

impl Default for ShimConfig {
    fn default() -> Self {
        Self {
            real_git: default_real_git(),
            real_gh: default_real_gh(),
        }
    }
}

pub const DEFAULT_CONFIG_PATH: &str = "/etc/ghbrk/config.yaml";

/// Load config from `GHBRK_CONFIG` env var or the default path.
/// Returns `Ok(ShimConfig)` on success, or `Err((resolved_path, error))` on failure.
pub fn load() -> Result<ShimConfig, (String, Box<dyn std::error::Error>)> {
    let path = std::env::var("GHBRK_CONFIG")
        .ok()
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
    load_from(&path).map_err(|e| (path, e))
}

pub fn load_from(path: impl AsRef<Path>) -> Result<ShimConfig, Box<dyn std::error::Error>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(ShimConfig::default());
    }
    let contents = std::fs::read_to_string(path)?;
    let config: ShimConfig = serde_yaml::from_str(&contents)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn missing_file_uses_defaults() {
        let cfg = load_from("/tmp/ghbrk-nonexistent-config-xyz.yaml").unwrap();
        assert_eq!(cfg.real_git, "/usr/bin/git");
        assert_eq!(cfg.real_gh, "/usr/bin/gh");
    }

    #[test]
    fn config_overrides_both_paths() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "real_git: /usr/local/bin/git").unwrap();
        writeln!(f, "real_gh: /usr/local/bin/gh").unwrap();
        let cfg = load_from(f.path()).unwrap();
        assert_eq!(cfg.real_git, "/usr/local/bin/git");
        assert_eq!(cfg.real_gh, "/usr/local/bin/gh");
    }

    #[test]
    fn config_single_field_default_other() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "real_git: /opt/git/bin/git").unwrap();
        let cfg = load_from(f.path()).unwrap();
        assert_eq!(cfg.real_git, "/opt/git/bin/git");
        assert_eq!(cfg.real_gh, "/usr/bin/gh");
    }

    #[test]
    fn malformed_config_errors() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "real_git: [not a string").unwrap();
        let result = load_from(f.path());
        assert!(result.is_err());
    }
}
