//! Tor v3 onion service **client authorization** (cahier: hidden services with
//! a client key). Lets the companion register a client private key with Tor so
//! a restricted `.onion` becomes reachable.
//!
//! This module handles the *crypto + control-port* side:
//!   - generate an x25519 client key pair,
//!   - normalize a pasted key (base32 / base64 / full `.auth_private` line),
//!   - inject / remove the key via `ONION_CLIENT_AUTH_ADD` / `_REMOVE`.
//!
//! Storage (OS keystore / passphrase vault) and the extension UI live
//! elsewhere; injection is always non-permanent (re-applied each session).

use anyhow::{anyhow, bail, Result};
use data_encoding::{BASE32_NOPAD, BASE64, BASE64_NOPAD};
use rand::RngCore;
use tokio::time::{timeout, Duration};

use crate::tor_detector;

const CONTROL_OP_TIMEOUT: Duration = Duration::from_secs(5);

/// Generate a fresh x25519 client key pair.
///
/// Returns `(private_key_base64, public_descriptor)`:
///   - the private key (base64) is what we feed to `ONION_CLIENT_AUTH_ADD`,
///   - the public descriptor `descriptor:x25519:<base32>` is what the user
///     hands to the service operator for their `authorized_clients/`.
pub fn generate_keypair() -> (String, String) {
    use x25519_dalek::{PublicKey, StaticSecret};
    let mut sk = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut sk);
    let secret = StaticSecret::from(sk);
    let public = PublicKey::from(&secret);
    let priv_b64 = BASE64.encode(&secret.to_bytes());
    let pub_descriptor = format!("descriptor:x25519:{}", BASE32_NOPAD.encode(public.as_bytes()));
    (priv_b64, pub_descriptor)
}

/// Normalize a user-pasted credential into `(maybe_onion, private_key_base64)`.
///
/// Accepts:
///   - a full `.auth_private` line `<addr>:descriptor:x25519:<base32>`
///     (the onion address is extracted),
///   - a bare key in base32 (52 chars) or base64 (44 chars).
pub fn normalize_private_key(input: &str) -> Result<(Option<String>, String)> {
    let s = input.trim();

    if let Some(idx) = s.find(":descriptor:x25519:") {
        let addr = &s[..idx];
        let key = s[idx + ":descriptor:x25519:".len()..].trim();
        let bytes = decode_key(key).ok_or_else(|| anyhow!("invalid key in .auth_private line"))?;
        return Ok((normalize_onion(addr).ok(), BASE64.encode(&bytes)));
    }

    let bytes = decode_key(s).ok_or_else(|| anyhow!("could not decode key as base32 or base64"))?;
    Ok((None, BASE64.encode(&bytes)))
}

/// Decode a 32-byte x25519 key from base32 or base64 (padded or not).
fn decode_key(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();

    // base32 (RFC 4648, uppercase) -- 32 bytes -> 52 chars.
    if let Ok(b) = BASE32_NOPAD.decode(s.to_ascii_uppercase().as_bytes()) {
        if let Ok(arr) = <[u8; 32]>::try_from(b.as_slice()) {
            return Some(arr);
        }
    }
    // base64 (standard alphabet), tolerate padding.
    if let Ok(b) = BASE64_NOPAD.decode(s.trim_end_matches('=').as_bytes()) {
        if let Ok(arr) = <[u8; 32]>::try_from(b.as_slice()) {
            return Some(arr);
        }
    }
    None
}

/// Validate a v3 onion address and return it without the `.onion` suffix
/// (the form `ONION_CLIENT_AUTH_*` expects).
pub fn normalize_onion(addr: &str) -> Result<String> {
    let a = addr.trim().to_ascii_lowercase();
    let core = a.strip_suffix(".onion").unwrap_or(&a);
    let valid = core.len() == 56 && core.bytes().all(|c| matches!(c, b'a'..=b'z' | b'2'..=b'7'));
    if valid {
        Ok(core.to_string())
    } else {
        bail!("not a valid v3 .onion address")
    }
}

/// Register a client key for `onion` with the Tor at `control_port`
/// (non-permanent: cleared on Tor restart, re-applied next session).
pub async fn add(control_port: u16, onion: &str, private_key_b64: &str) -> Result<()> {
    let onion = normalize_onion(onion)?;
    timeout(CONTROL_OP_TIMEOUT, async {
        let (mut r, mut w) = tor_detector::connect_and_auth(control_port).await?;
        let cmd = format!("ONION_CLIENT_AUTH_ADD {onion} x25519:{private_key_b64}");
        let ok = tor_detector::send_command(&mut r, &mut w, &cmd).await?;
        let _ = tor_detector::send_command(&mut r, &mut w, "QUIT").await;
        if ok {
            Ok(())
        } else {
            bail!("Tor rejected ONION_CLIENT_AUTH_ADD (bad key or address?)")
        }
    })
    .await
    .map_err(|_| anyhow!("control port timed out"))?
}

/// Remove a previously-registered client key.
pub async fn remove(control_port: u16, onion: &str) -> Result<()> {
    let onion = normalize_onion(onion)?;
    timeout(CONTROL_OP_TIMEOUT, async {
        let (mut r, mut w) = tor_detector::connect_and_auth(control_port).await?;
        let ok = tor_detector::send_command(&mut r, &mut w, &format!("ONION_CLIENT_AUTH_REMOVE {onion}"))
            .await?;
        let _ = tor_detector::send_command(&mut r, &mut w, "QUIT").await;
        if ok {
            Ok(())
        } else {
            bail!("Tor rejected ONION_CLIENT_AUTH_REMOVE")
        }
    })
    .await
    .map_err(|_| anyhow!("control port timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keygen_roundtrips() {
        let (priv_b64, descriptor) = generate_keypair();
        // private key decodes to 32 bytes
        assert!(decode_key(&priv_b64).is_some());
        // descriptor is well-formed and its base32 pubkey is 32 bytes
        let pub_b32 = descriptor.strip_prefix("descriptor:x25519:").unwrap();
        assert_eq!(BASE32_NOPAD.decode(pub_b32.as_bytes()).unwrap().len(), 32);
    }

    #[test]
    fn decodes_base32_and_base64_to_same_bytes() {
        let raw = [7u8; 32];
        let b32 = BASE32_NOPAD.encode(&raw);
        let b64 = BASE64.encode(&raw);
        assert_eq!(decode_key(&b32), Some(raw));
        assert_eq!(decode_key(&b64), Some(raw));
        assert_eq!(decode_key(b64.trim_end_matches('=')), Some(raw)); // no padding
    }

    #[test]
    fn parses_full_auth_private_line() {
        let raw = [9u8; 32];
        let addr = "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd";
        let line = format!("{addr}:descriptor:x25519:{}", BASE32_NOPAD.encode(&raw));
        let (onion, key) = normalize_private_key(&line).unwrap();
        assert_eq!(onion.as_deref(), Some(addr));
        assert_eq!(decode_key(&key), Some(raw));
    }

    #[test]
    fn parses_bare_key_without_address() {
        let raw = [3u8; 32];
        let (onion, key) = normalize_private_key(&BASE64.encode(&raw)).unwrap();
        assert!(onion.is_none());
        assert_eq!(decode_key(&key), Some(raw));
    }

    #[test]
    fn onion_validation() {
        let good = "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd";
        assert_eq!(normalize_onion(&format!("{good}.onion")).unwrap(), good);
        assert_eq!(normalize_onion(good).unwrap(), good);
        assert!(normalize_onion("too-short.onion").is_err());
        assert!(normalize_onion("UPPER0123.onion").is_err());
    }

    /// Network test: inject + remove a client key against a live Tor control
    /// port (defaults to 9051). Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a running Tor control port on 9051"]
    async fn add_remove_against_live_tor() {
        // A real, checksum-valid v3 address (Tor Project) -- the bogus key just
        // creates a local mapping; we remove it right after.
        let onion = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid";
        let (priv_b64, _) = generate_keypair();
        add(9051, onion, &priv_b64).await.expect("ADD should succeed");
        remove(9051, onion).await.expect("REMOVE should succeed");
    }
}
