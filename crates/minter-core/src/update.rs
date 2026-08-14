//! Release update check against GitHub.
//!
//! The desktop build ships as an unsigned binary people download by hand, so
//! nothing tells them a newer one exists. This asks the Releases API once and
//! reports whether the running version is behind.
//!
//! Deliberately unobtrusive:
//! * it is a plain outbound GET to api.github.com and sends nothing about the
//!   operator — no wallet data, no identifiers, not even a query string;
//! * every failure (offline, rate limited, malformed) resolves to "no update
//!   known" rather than an error the operator has to dismiss;
//! * it never downloads or installs anything. The result is a version string
//!   and a link.

use anyhow::{Context, Result};
use serde::Serialize;
use std::time::Duration;

/// Repository queried for releases.
pub const DEFAULT_REPO: &str = "MaxBetov-pdd/Minter-rs-v2";

/// Version this binary was built from.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    /// Latest published tag, without the leading `v`. Empty when unknown.
    pub latest: String,
    pub update_available: bool,
    /// Release page to open. Empty when unknown.
    pub url: String,
    /// Human note for the UI — why there is no answer, when there is none.
    pub note: Option<String>,
}

impl UpdateInfo {
    fn unknown(reason: impl Into<String>) -> Self {
        Self {
            current: current_version().to_string(),
            latest: String::new(),
            update_available: false,
            url: String::new(),
            note: Some(reason.into()),
        }
    }
}

/// Parse `v1.2.3` / `1.2.3` / `1.2` into comparable numbers.
///
/// Anything unparseable yields `None` so an odd tag can never be read as
/// "newer" and nag the operator forever.
fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let s = raw.trim().trim_start_matches(['v', 'V']);
    // Drop any pre-release / build suffix: 1.2.3-rc1+build → 1.2.3
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// True when `latest` is strictly newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        // Unknown on either side: stay quiet rather than guess.
        _ => false,
    }
}

/// Ask GitHub for the latest release. Never returns an error the caller has to
/// handle — problems come back as an `UpdateInfo` with a note.
pub async fn check_for_update(repo: &str) -> UpdateInfo {
    match fetch_latest(repo).await {
        Ok((tag, url)) => {
            let latest = tag.trim_start_matches(['v', 'V']).to_string();
            let current = current_version().to_string();
            UpdateInfo {
                update_available: is_newer(&latest, &current),
                current,
                latest,
                url,
                note: None,
            }
        }
        Err(e) => UpdateInfo::unknown(format!("update check unavailable: {e}")),
    }
}

async fn fetch_latest(repo: &str) -> Result<(String, String)> {
    let client = reqwest::Client::builder()
        // Short: this runs at startup and must never hold the UI back.
        .timeout(Duration::from_secs(8))
        .build()
        .context("build update client")?;
    let resp = client
        .get(format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        // GitHub rejects requests without one.
        .header("user-agent", format!("minter/{}", current_version()))
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .context("reach github")?;
    if !resp.status().is_success() {
        anyhow::bail!("github returned HTTP {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await.context("parse release json")?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .context("release has no tag_name")?
        .to_string();
    let url = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    Ok((tag, url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_tag_shapes() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version(" 0.2.0 "), Some((0, 2, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("2"), Some((2, 0, 0)));
        // Pre-release and build metadata are ignored, not rejected.
        assert_eq!(parse_version("1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3+build7"), Some((1, 2, 3)));
    }

    #[test]
    fn rejects_nonsense_rather_than_guessing() {
        for s in ["", "latest", "v", "x.y.z", "nightly"] {
            assert_eq!(parse_version(s), None, "{s}");
        }
    }

    #[test]
    fn compares_by_number_not_by_text() {
        // The case a string compare gets wrong: "0.10.0" < "0.9.0" as text.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("v0.2.1", "0.2.0"));
    }

    #[test]
    fn equal_or_older_is_not_an_update() {
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        // A local build ahead of the published tag must not be told to
        // "update" back down to it.
        assert!(!is_newer("0.2.0", "0.3.0"));
    }

    #[test]
    fn unparseable_never_prompts() {
        assert!(!is_newer("garbage", "0.2.0"));
        assert!(!is_newer("0.3.0", "garbage"));
    }

    #[test]
    fn unknown_carries_a_note_and_no_prompt() {
        let u = UpdateInfo::unknown("offline");
        assert!(!u.update_available);
        assert!(u.latest.is_empty());
        assert_eq!(u.note.as_deref(), Some("offline"));
        assert_eq!(u.current, current_version());
    }
}
