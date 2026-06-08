//! Check whether a newer **companion** version has been released, routing the
//! request through Tor so the user's real IP never reaches GitHub.
//!
//! The extension auto-updates via AMO; the companion does not, so it's the
//! companion's job to tell the user when a newer installer is available.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

const RELEASES_URL: &str =
    "https://api.github.com/repos/LouisCourrian/OnionRooter/releases?per_page=30";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
}

/// Result handed back to the extension.
#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    /// Currently-running companion version.
    pub current: String,
    /// Latest released companion version, if any was found.
    pub latest: Option<String>,
    /// Whether `latest` is newer than `current`.
    pub update_available: bool,
    /// GitHub release page for the latest version.
    pub url: Option<String>,
}

/// Query GitHub for the latest stable `companion-v*` release **via Tor**.
pub async fn check(socks_port: u16) -> Result<UpdateInfo> {
    let proxy = reqwest::Proxy::all(format!("socks5h://127.0.0.1:{socks_port}"))
        .context("building SOCKS proxy")?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .user_agent("OnionRouter-companion")
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;

    let releases: Vec<Release> = client
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("GET releases via Tor")?
        .error_for_status()
        .context("GitHub returned an error")?
        .json()
        .await
        .context("parsing releases JSON")?;

    let current = env!("CARGO_PKG_VERSION").to_string();
    let current_v = parse_semver(&current);

    let mut best: Option<((u32, u32, u32), String, String)> = None;
    for r in releases {
        if r.prerelease {
            continue;
        }
        let Some(ver) = r.tag_name.strip_prefix("companion-v") else {
            continue;
        };
        if let Some(v) = parse_semver(ver) {
            if best.as_ref().map_or(true, |(b, _, _)| v > *b) {
                best = Some((v, ver.to_string(), r.html_url));
            }
        }
    }

    Ok(match best {
        Some((v, s, u)) => UpdateInfo {
            update_available: current_v.map_or(false, |c| v > c),
            current,
            latest: Some(s),
            url: Some(u),
        },
        None => UpdateInfo {
            update_available: false,
            current,
            latest: None,
            url: None,
        },
    })
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    // Take the leading "X.Y.Z", ignoring any "-suffix".
    let core: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut it = core.split('.').map(|p| p.parse::<u32>());
    let a = it.next()?.ok()?;
    let b = it.next().transpose().ok()?.unwrap_or(0);
    let c = it.next().transpose().ok()?.unwrap_or(0);
    Some((a, b, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing() {
        assert_eq!(parse_semver("1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_semver("0.5.10-rc1"), Some((0, 5, 10)));
        assert!(parse_semver("nope").is_none());
        assert!((1, 0, 0) > (0, 9, 9));
    }
}
