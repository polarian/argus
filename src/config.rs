//! Load TOML config file.
//!
//! Lookup order (uses the first file found):
//!   1. ./argus.toml                         (per-project config)
//!   2. $XDG_CONFIG_HOME/argus/config.toml    (falls back to ~/.config/argus/config.toml)
//!   3. ~/.argus.toml
//!
//! All values are optional, and CLI args take precedence over config.
//!
//! Example argus.toml:
//! ```toml
//! repo = "cli/cli"
//! poll_secs = 10
//! limit = 30
//! ```

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Default repository to monitor. GitHub uses `owner/repo`, Bitbucket uses `workspace/repo_slug`.
    pub repo: Option<String>,
    /// Auto-refresh interval (seconds).
    pub poll_secs: Option<u64>,
    /// Number of items to display/fetch per panel.
    pub limit: Option<usize>,
    /// Color theme name (default · nord · catppuccin-mocha · dracula · tokyo-night).
    pub theme: Option<String>,
    /// Data source backend (`github`=gh · `bitbucket`/`bb`=bkt). Defaults to github when unset.
    pub backend: Option<String>,
    /// UI language (`en` · `ko`). Defaults to locale auto-detection when unset.
    pub lang: Option<String>,
    /// Whether to check GitHub for a newer release on startup (default true).
    pub update_check: Option<bool>,
}

impl Config {
    /// Walks the candidate paths and returns the first config that reads successfully.
    /// Returns defaults if no file is found, or warns and returns defaults on parse failure.
    pub fn load() -> Self {
        for path in candidate_paths() {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match toml::from_str::<Config>(&text) {
                Ok(cfg) => return cfg,
                Err(e) => {
                    eprintln!("arg: config parse failed {}: {e}", path.display());
                    return Config::default();
                }
            }
        }
        Config::default()
    }
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("argus.toml")];

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        paths.push(xdg.join("argus").join("config.toml"));
        paths.push(home.join(".argus.toml"));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_full_config() {
        let c: Config =
            toml::from_str("repo = \"a/b\"\npoll_secs = 10\nlimit = 30\nbackend = \"bitbucket\"")
                .unwrap();
        assert_eq!(c.repo.as_deref(), Some("a/b"));
        assert_eq!(c.poll_secs, Some(10));
        assert_eq!(c.limit, Some(30));
        assert_eq!(c.backend.as_deref(), Some("bitbucket"));
    }

    #[test]
    fn parses_partial_and_empty() {
        let c: Config = toml::from_str("poll_secs = 5").unwrap();
        assert_eq!(c.repo, None);
        assert_eq!(c.poll_secs, Some(5));
        assert_eq!(c.limit, None);

        let empty: Config = toml::from_str("").unwrap();
        assert!(empty.repo.is_none() && empty.poll_secs.is_none() && empty.limit.is_none());
    }

    #[test]
    fn rejects_wrong_types() {
        // poll_secs must be an integer.
        assert!(toml::from_str::<Config>("poll_secs = \"soon\"").is_err());
    }
}
