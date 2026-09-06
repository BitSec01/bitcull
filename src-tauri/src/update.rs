//! Checking GitHub for a newer release.
//!
//! Deliberately the *simple* half of updating: this asks whether a newer tag
//! exists and offers a link. It does not download or install anything.
//!
//! Tauri does ship a real auto-updater, and it works well, but it requires
//! generating a minisign keypair, keeping the private key in CI secrets, and
//! signing every artefact. For an open-source tool whose users are happy to
//! click a download link once in a while, that is a lot of key management to
//! own for little gain — and a compromised key is a much worse failure than a
//! missed update. Start here; move to `tauri-plugin-updater` if silent updates
//! ever become worth the ceremony.

use anyhow::{anyhow, Result};
use serde::Serialize;

/// `owner/repo` to check. Change this to your own fork.
pub const REPO: &str = "BitSec01/bitcull";

/// How long to wait before giving up. A version check must never make the app
/// feel slow, and failing to reach GitHub is not an error worth surfacing.
const TIMEOUT_SECS: u64 = 6;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    /// True only when `latest` is genuinely newer than `current`.
    pub newer: bool,
    /// Release page to send the user to.
    pub url: String,
    pub notes: String,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Ask GitHub for the latest release.
pub fn check() -> Result<UpdateInfo> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        // GitHub rejects requests without a User-Agent.
        .set("User-Agent", "culling-update-check")
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .call()
        .map_err(|e| anyhow!("could not reach GitHub: {e}"))?;

    let json: serde_json::Value = resp.into_json()?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("release has no tag_name"))?;

    let current = current_version();
    Ok(UpdateInfo {
        newer: is_newer(tag, current),
        current: current.to_string(),
        latest: tag.trim_start_matches('v').to_string(),
        url: json
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or(&format!("https://github.com/{REPO}/releases"))
            .to_string(),
        notes: json
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .chars()
            .take(600)
            .collect(),
    })
}

/// Compare two dotted versions, tolerating a leading `v` and trailing
/// pre-release suffixes.
///
/// Deliberately conservative: anything it cannot parse is treated as "not
/// newer", because nagging a user about a bogus update is worse than staying
/// quiet.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<Vec<u32>> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        // Drop any pre-release / build suffix: 1.2.0-beta.1 -> 1.2.0
        let core = s.split(['-', '+']).next()?;
        let parts: Vec<u32> = core
            .split('.')
            .map(|p| p.parse::<u32>().ok())
            .collect::<Option<Vec<_>>>()?;
        if parts.is_empty() {
            None
        } else {
            Some(parts)
        }
    };

    let (Some(a), Some(b)) = (parse(candidate), parse(current)) else {
        return false;
    };

    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn detects_a_newer_release() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"), "numeric, not lexical");
        assert!(is_newer("0.2", "0.1.9"), "shorter but higher");
    }

    #[test]
    fn ignores_same_or_older() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.1.0.0"));
    }

    #[test]
    fn treats_prerelease_core_as_the_version() {
        assert!(is_newer("0.2.0-beta.1", "0.1.0"));
        // Same core version: a beta is not an upgrade over the release.
        assert!(!is_newer("0.1.0-beta.1", "0.1.0"));
    }

    /// Anything unparseable must stay silent rather than nag.
    #[test]
    fn unparseable_is_never_newer() {
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("0.2.0", "not-a-version"));
    }
}
