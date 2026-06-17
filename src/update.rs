//! Update notifier (notify only — no self-replace).
//!
//! Queries argus's own GitHub releases via `curl` (network delegated to the CLI,
//! no extra Rust deps) and compares the latest tag with the running version. On a
//! newer version it sends `DataMsg::Update` so the UI can show a banner. Any
//! failure (offline, private repo, rate limit, no curl) is ignored silently.
//! Opt out via `ARGUS_NO_UPDATE_CHECK` or `update_check = false` in the config.

use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::github::DataMsg;

const REPO: &str = "polarian/argus";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// One-shot check: if a newer release exists, send `DataMsg::Update(version)`.
pub async fn check(tx: mpsc::Sender<DataMsg>) {
    if let Some(v) = latest_newer().await {
        let _ = tx.send(DataMsg::Update(v)).await;
    }
}

/// Returns the latest release version (without the `v`) if it is newer than the
/// running build, else None.
async fn latest_newer() -> Option<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = Command::new("curl")
        .args(["-fsSL", "-H", "Accept: application/vnd.github+json", &url])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let tag = parse_tag(&out.stdout)?;
    let latest = tag.trim_start_matches('v');
    if is_newer(latest, env!("CARGO_PKG_VERSION")) {
        Some(latest.to_string())
    } else {
        None
    }
}

/// Extract `tag_name` from the GitHub release JSON.
fn parse_tag(bytes: &[u8]) -> Option<String> {
    serde_json::from_slice::<Release>(bytes)
        .ok()
        .map(|r| r.tag_name)
}

/// Dotted-numeric version compare: is `a` strictly newer than `b`?
/// Non-numeric components are treated as 0 (good enough for `x.y.z`).
fn is_newer(a: &str, b: &str) -> bool {
    let parts = |s: &str| {
        s.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::{is_newer, parse_tag};

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.1.3", "0.1.2"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(!is_newer("0.1.1", "0.1.2"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn parses_release_tag() {
        let json = br#"{"tag_name":"v0.1.3","name":"v0.1.3"}"#;
        assert_eq!(parse_tag(json).as_deref(), Some("v0.1.3"));
        assert_eq!(parse_tag(b"not json"), None);
    }
}
