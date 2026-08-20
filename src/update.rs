//! Update check: asks GitHub for the project's latest release tag.
//!
//! Read-only and opt-out-free but harmless: one HTTPS GET every 8 hours while
//! the app is running. Quotty never downloads or installs anything by itself —
//! it only points at the release page.

use serde::Deserialize;

pub const RELEASES_PAGE: &str = "https://github.com/confeden/Quotty/releases/latest";
const API_URL: &str = "https://api.github.com/repos/confeden/Quotty/releases/latest";

/// How long between automatic checks.
pub const CHECK_EVERY_SECS: u64 = 8 * 3600;

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Clone, Debug)]
pub struct Update {
    pub version: String,
    pub url: String,
}

#[derive(Default)]
pub struct UpdateState {
    /// A check has completed at least once.
    pub checked: bool,
    /// Set only when the published release is newer than this build.
    pub available: Option<Update>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    tag_name: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// `Ok(None)` = we are up to date (or the repo has no published release yet).
pub fn check() -> Result<Option<Update>, String> {
    let resp = match ureq::get(API_URL)
        .set("User-Agent", &format!("Quotty/{}", current()))
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .call()
    {
        Ok(r) => r,
        // No releases published yet — nothing to report, and not a failure.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(code, _)) => return Err(format!("status {code}")),
        Err(e) => return Err(format!("network: {e}")),
    };

    let release: Release = resp.into_json().map_err(|e| format!("parse: {e}"))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let Some(tag) = release.tag_name else {
        return Ok(None);
    };
    if !is_newer(&tag, current()) {
        return Ok(None);
    }
    Ok(Some(Update {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: release
            .html_url
            .unwrap_or_else(|| RELEASES_PAGE.to_string()),
    }))
}

/// Numeric, component-wise comparison of `v1.2.3`-style versions. Anything we
/// can't parse counts as "not newer", so a stray tag never nags the user.
fn is_newer(tag: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.trim()
            .trim_start_matches(['v', 'V'])
            .split(['.', '-', '+'])
            .map_while(|p| p.parse::<u32>().ok())
            .collect()
    };
    let (new, cur) = (parse(tag), parse(current));
    if new.is_empty() {
        return false;
    }
    for i in 0..new.len().max(cur.len()) {
        let a = new.get(i).copied().unwrap_or(0);
        let b = cur.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v1.1.0", "1.0.0"));
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("2.0", "1.9.9"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.9", "1.0.0"));
        assert!(!is_newer("nightly", "1.0.0"));
    }
}
