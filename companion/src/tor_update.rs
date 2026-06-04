//! Optional, secure auto-update of the Tor Expert Bundle (cahier F11).
//!
//! The Tor Project prunes old versions from `dist.torproject.org`, so a single
//! hard-pinned version eventually 404s. To stay current without weakening the
//! "verified hash" guarantee, this module:
//!
//!   1. discovers the latest stable Tor Browser version,
//!   2. downloads `sha256sums-signed-build.txt` and its detached `.asc`,
//!   3. **verifies the PGP signature** against the Tor Browser build signing
//!      key pinned in the binary (`assets/tor-signing-key.asc`),
//!   4. extracts the SHA-256 for the host platform's expert bundle.
//!
//! The pinned [`tor_manager::BUNDLE_VERSION`] always remains a guaranteed
//! fallback: any failure here (offline, parse error, signature mismatch)
//! falls back to it, so the companion never breaks because of auto-update.

use std::io::Cursor;

use anyhow::{anyhow, bail, Context, Result};
use tracing::{info, warn};

use crate::tor_manager::{self, ResolvedBundle};

/// Tor Browser build signing key (primary EF6E286D…93298290, signing subkey
/// CAAE408A…78A65729). Fetched via WKD from `torbrowser@torproject.org` and
/// embedded so verification needs no network and no local GnuPG.
const TOR_SIGNING_KEY: &str = include_str!("../assets/tor-signing-key.asc");

const DIST_ROOT: &str = "https://dist.torproject.org/torbrowser";

/// Resolve which bundle to install: the latest PGP-verified version if we can
/// reach and verify it, otherwise the pinned fallback. Never fails.
pub async fn resolve_bundle() -> ResolvedBundle {
    let pinned = match tor_manager::pinned_bundle() {
        Ok(p) => p,
        Err(e) => {
            // Unsupported platform etc. -- nothing we can do here; hand back a
            // best-effort error bundle so the caller surfaces it normally.
            warn!("no pinned bundle for host: {e:#}");
            return ResolvedBundle::unsupported();
        }
    };

    match try_latest(&pinned).await {
        Ok(Some(rb)) => {
            info!("auto-update: using verified latest Tor {}", rb.version);
            rb
        }
        Ok(None) => {
            info!("auto-update: pinned Tor {} is up to date", pinned.version);
            pinned
        }
        Err(e) => {
            warn!(
                "auto-update unavailable ({e:#}); falling back to pinned Tor {}",
                pinned.version
            );
            pinned
        }
    }
}

async fn try_latest(pinned: &ResolvedBundle) -> Result<Option<ResolvedBundle>> {
    let latest = discover_latest().await?;
    if !version_gt(&latest, &pinned.version) {
        return Ok(None);
    }

    let base = format!("{DIST_ROOT}/{latest}");
    let sums = fetch_text(&format!("{base}/sha256sums-signed-build.txt")).await?;
    let sig = fetch_bytes(&format!("{base}/sha256sums-signed-build.txt.asc")).await?;

    verify_signature(sums.as_bytes(), &sig)
        .context("PGP verification of Tor sha256sums failed")?;

    let platform = pinned.platform;
    let archive = format!("tor-expert-bundle-{platform}-{latest}.tar.gz");
    let sha256 = hash_for(&sums, &archive)
        .ok_or_else(|| anyhow!("no {archive} entry in signed sums"))?;

    Ok(Some(ResolvedBundle {
        version: latest,
        url: format!("{base}/{archive}"),
        sha256,
        binary_subpath: pinned.binary_subpath,
        platform,
    }))
}

/// Parse the directory listing and return the highest stable version string
/// (alphas like `16.0a6` are ignored -- they contain non-digit characters).
async fn discover_latest() -> Result<String> {
    let html = fetch_text(&format!("{DIST_ROOT}/")).await?;
    let mut best: Option<((u32, u32, u32), String)> = None;
    for piece in html.split("href=\"").skip(1) {
        let Some(end) = piece.find('"') else { continue };
        let href = piece[..end].trim_end_matches('/');
        if href.is_empty() || !href.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        if let Some(v) = parse_version(href) {
            if best.as_ref().map_or(true, |(b, _)| v > *b) {
                best = Some((v, href.to_string()));
            }
        }
    }
    best.map(|(_, s)| s)
        .ok_or_else(|| anyhow!("no stable version found in dist listing"))
}

fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.split('.').map(|p| p.parse::<u32>());
    let a = it.next()?.ok()?;
    let b = it.next().transpose().ok()?.unwrap_or(0);
    let c = it.next().transpose().ok()?.unwrap_or(0);
    Some((a, b, c))
}

fn version_gt(a: &str, b: &str) -> bool {
    match (parse_version(a), parse_version(b)) {
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
}

/// Find the lowercase hex SHA-256 for a given archive name in a sums file.
fn hash_for(sums: &str, archive: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim();
        if line.ends_with(archive) {
            if let Some(hash) = line.split_whitespace().next() {
                if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(hash.to_lowercase());
                }
            }
        }
    }
    None
}

/// Verify a detached PGP signature over `content` against the pinned key.
/// Tries the primary key and every subkey (the sums are signed by a subkey).
fn verify_signature(content: &[u8], sig_armored: &[u8]) -> Result<()> {
    use pgp::composed::{Deserializable, DetachedSignature, SignedPublicKey};

    let (key, _) = SignedPublicKey::from_armor_single(Cursor::new(TOR_SIGNING_KEY.as_bytes()))
        .context("parsing pinned Tor signing key")?;
    let (sig, _) = DetachedSignature::from_armor_single(Cursor::new(sig_armored))
        .context("parsing detached signature")?;

    if sig.verify(&key, content).is_ok() {
        return Ok(());
    }
    for sub in &key.public_subkeys {
        if sig.verify(sub, content).is_ok() {
            return Ok(());
        }
    }
    bail!("signature does not verify against the pinned Tor key");
}

async fn fetch_text(url: &str) -> Result<String> {
    reqwest::get(url)
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error for {url}"))?
        .text()
        .await
        .context("reading response text")
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    Ok(reqwest::get(url)
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error for {url}"))?
        .bytes()
        .await
        .context("reading response bytes")?
        .to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_highest_stable_ignores_alpha() {
        let html = r#"
            <a href="15.0.13/">15.0.13/</a>
            <a href="15.0.15/">15.0.15/</a>
            <a href="16.0a6/">16.0a6/</a>
            <a href="../">../</a>
        "#;
        // Reuse the same parsing logic discover_latest uses.
        let mut best: Option<((u32, u32, u32), String)> = None;
        for piece in html.split("href=\"").skip(1) {
            let end = piece.find('"').unwrap();
            let href = piece[..end].trim_end_matches('/');
            if href.is_empty() || !href.chars().all(|c| c.is_ascii_digit() || c == '.') {
                continue;
            }
            if let Some(v) = parse_version(href) {
                if best.as_ref().map_or(true, |(b, _)| v > *b) {
                    best = Some((v, href.to_string()));
                }
            }
        }
        assert_eq!(best.unwrap().1, "15.0.15");
    }

    #[test]
    fn version_compare() {
        assert!(version_gt("15.0.15", "15.0.13"));
        assert!(version_gt("15.1.0", "15.0.99"));
        assert!(!version_gt("15.0.13", "15.0.15"));
        assert!(!version_gt("15.0.15", "15.0.15"));
    }

    /// Network test: the embedded key must verify the live signed sums, and a
    /// tampered copy must be rejected. Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "network access required"]
    async fn verifies_real_tor_signature() {
        let base = format!("{DIST_ROOT}/15.0.15");
        let sums = fetch_text(&format!("{base}/sha256sums-signed-build.txt"))
            .await
            .unwrap();
        let sig = fetch_bytes(&format!("{base}/sha256sums-signed-build.txt.asc"))
            .await
            .unwrap();

        verify_signature(sums.as_bytes(), &sig)
            .expect("embedded key must verify the real Tor sums");

        let mut tampered = sums.into_bytes();
        tampered[0] ^= 0xff;
        assert!(
            verify_signature(&tampered, &sig).is_err(),
            "tampered content must NOT verify"
        );
    }

    #[test]
    fn extracts_hash_for_archive() {
        let sums = "\
deadbeef00000000000000000000000000000000000000000000000000000000  tor-expert-bundle-linux-x86_64-15.0.15.tar.gz
8d3daf579192f3f128c0f42553dd994c640501b4b98682216d807c88004f7a96  tor-expert-bundle-windows-x86_64-15.0.15.tar.gz";
        assert_eq!(
            hash_for(sums, "tor-expert-bundle-windows-x86_64-15.0.15.tar.gz").as_deref(),
            Some("8d3daf579192f3f128c0f42553dd994c640501b4b98682216d807c88004f7a96")
        );
        assert!(hash_for(sums, "tor-expert-bundle-macos-x86_64-15.0.15.tar.gz").is_none());
    }
}
