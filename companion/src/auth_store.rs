//! Persistent storage for v3 onion client-auth credentials (0.5.0).
//!
//! Two tiers, chosen per credential:
//!   - **OS** (`os-vault.bin`): encrypted with Windows DPAPI (user-scoped).
//!     Windows-only; auto-decryptable while the user is logged in.
//!   - **passphrase** (`pp-vault.bin`): Argon2id-derived key + XChaCha20-Poly1305.
//!     Cross-platform; opaque until the user unlocks with their passphrase.
//!
//! A plaintext `index.json` lists non-secret metadata so the UI can show the
//! list and the extension can detect protected `.onion`s even while locked.
//! For privacy, passphrase entries store only the **SHA-256 of the address**,
//! never the address itself.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// A stored credential secret (lives only inside an encrypted vault).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Secret {
    pub onion: String,
    pub label: String,
    pub privkey_b64: String,
}

/// Non-secret metadata kept in the plaintext index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub tier: String, // "os" | "passphrase"
    pub label: String,
    /// Present for OS-tier entries (already auto-loaded, so not secret here).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onion: Option<String>,
    /// SHA-256 hex of the address, for passphrase-tier entries (privacy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onion_hash: Option<String>,
}

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24; // XChaCha20-Poly1305 nonce
const VAULT_VERSION: u8 = 1;

fn dir() -> Result<PathBuf> {
    // ONIONROUTER_DATA_DIR overrides the OnionRouter base dir (tests, portable
    // installs); otherwise the per-user local data dir is used.
    let base = match std::env::var_os("ONIONROUTER_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => dirs::data_local_dir()
            .ok_or_else(|| anyhow!("no local data dir"))?
            .join("OnionRouter"),
    };
    let d = base.join("client-auth");
    std::fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    Ok(d)
}

fn index_path() -> Result<PathBuf> {
    Ok(dir()?.join("index.json"))
}
fn os_vault_path() -> Result<PathBuf> {
    Ok(dir()?.join("os-vault.bin"))
}
fn pp_vault_path() -> Result<PathBuf> {
    Ok(dir()?.join("pp-vault.bin"))
}

/// SHA-256 hex of a (normalized) onion address.
pub fn onion_hash(onion: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(onion.as_bytes());
    hex::encode(h.finalize())
}

/// Whether the OS keystore tier is available on this platform.
pub fn os_available() -> bool {
    cfg!(windows)
}

/// Whether a passphrase vault has been initialized.
pub fn vault_exists() -> bool {
    pp_vault_path().map(|p| p.exists()).unwrap_or(false)
}

// ---------- Index ------------------------------------------------------

pub fn list() -> Result<Vec<IndexEntry>> {
    let p = index_path()?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    Ok(serde_json::from_slice(&raw).unwrap_or_default())
}

fn write_index(entries: &[IndexEntry]) -> Result<()> {
    let p = index_path()?;
    std::fs::write(&p, serde_json::to_vec_pretty(entries)?)
        .with_context(|| format!("writing {}", p.display()))?;
    Ok(())
}

fn upsert_index(entry: IndexEntry) -> Result<()> {
    let mut entries = list()?;
    let key = entry.onion.clone();
    let hkey = entry.onion_hash.clone();
    entries.retain(|e| !(e.onion == key && key.is_some()) && !(e.onion_hash == hkey && hkey.is_some()));
    entries.push(entry);
    write_index(&entries)
}

/// Tier of a stored onion ("os" / "passphrase"), if known. Matches OS entries
/// by address and passphrase entries by hash.
pub fn tier_of(onion: &str) -> Result<Option<String>> {
    let h = onion_hash(onion);
    Ok(list()?.into_iter().find_map(|e| {
        if e.onion.as_deref() == Some(onion) || e.onion_hash.as_deref() == Some(h.as_str()) {
            Some(e.tier)
        } else {
            None
        }
    }))
}

// ---------- OS tier (DPAPI) --------------------------------------------

pub fn os_secrets() -> Result<Vec<Secret>> {
    let p = os_vault_path()?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let blob = std::fs::read(&p)?;
    let plain = dpapi_decrypt(&blob)?;
    Ok(serde_json::from_slice(&plain).unwrap_or_default())
}

pub fn os_add(onion: &str, label: &str, privkey_b64: &str) -> Result<()> {
    if !os_available() {
        bail!("OS keystore is only available on Windows");
    }
    let mut secrets = os_secrets().unwrap_or_default();
    secrets.retain(|s| s.onion != onion);
    secrets.push(Secret {
        onion: onion.to_string(),
        label: label.to_string(),
        privkey_b64: privkey_b64.to_string(),
    });
    let blob = dpapi_encrypt(&serde_json::to_vec(&secrets)?)?;
    std::fs::write(os_vault_path()?, blob)?;
    upsert_index(IndexEntry {
        tier: "os".into(),
        label: label.to_string(),
        onion: Some(onion.to_string()),
        onion_hash: None,
    })
}

// ---------- Passphrase tier (Argon2id + XChaCha20-Poly1305) -----------

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(key)
}

fn aead_encrypt(key: &[u8; 32], nonce: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| anyhow!("cipher: {e}"))?;
    cipher
        .encrypt(XNonce::from_slice(nonce), plaintext)
        .map_err(|_| anyhow!("encryption failed"))
}

fn aead_decrypt(key: &[u8; 32], nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| anyhow!("cipher: {e}"))?;
    cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| anyhow!("wrong passphrase or corrupt vault"))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    use rand::RngCore;
    let mut b = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

/// Read `[version|salt|nonce|ciphertext]`.
fn read_vault() -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let raw = std::fs::read(pp_vault_path()?).context("reading passphrase vault")?;
    if raw.len() < 1 + SALT_LEN + NONCE_LEN || raw[0] != VAULT_VERSION {
        bail!("passphrase vault is malformed");
    }
    let salt = raw[1..1 + SALT_LEN].to_vec();
    let nonce = raw[1 + SALT_LEN..1 + SALT_LEN + NONCE_LEN].to_vec();
    let ct = raw[1 + SALT_LEN + NONCE_LEN..].to_vec();
    Ok((salt, nonce, ct))
}

fn write_vault(salt: &[u8], key: &[u8; 32], secrets: &[Secret]) -> Result<()> {
    let nonce = random_bytes::<NONCE_LEN>();
    let ct = aead_encrypt(key, &nonce, &serde_json::to_vec(secrets)?)?;
    let mut out = Vec::with_capacity(1 + SALT_LEN + NONCE_LEN + ct.len());
    out.push(VAULT_VERSION);
    out.extend_from_slice(salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    std::fs::write(pp_vault_path()?, out)?;
    Ok(())
}

/// Initialize a new (empty) passphrase vault. Returns the derived key.
pub fn init_passphrase(passphrase: &str) -> Result<[u8; 32]> {
    if passphrase.is_empty() {
        bail!("passphrase must not be empty");
    }
    let salt = random_bytes::<SALT_LEN>();
    let key = derive_key(passphrase, &salt)?;
    write_vault(&salt, &key, &[])?;
    Ok(key)
}

/// Unlock the vault: returns the derived key and the decrypted secrets.
pub fn unlock(passphrase: &str) -> Result<([u8; 32], Vec<Secret>)> {
    let (salt, nonce, ct) = read_vault()?;
    let key = derive_key(passphrase, &salt)?;
    let plain = aead_decrypt(&key, &nonce, &ct)?;
    let secrets = serde_json::from_slice(&plain).unwrap_or_default();
    Ok((key, secrets))
}

/// Decrypt secrets using an already-derived key (held while unlocked).
pub fn pp_secrets(key: &[u8; 32]) -> Result<Vec<Secret>> {
    let (_salt, nonce, ct) = read_vault()?;
    let plain = aead_decrypt(key, &nonce, &ct)?;
    Ok(serde_json::from_slice(&plain).unwrap_or_default())
}

pub fn pp_add(key: &[u8; 32], onion: &str, label: &str, privkey_b64: &str) -> Result<()> {
    let (salt, nonce, ct) = read_vault()?;
    let plain = aead_decrypt(key, &nonce, &ct)?; // also verifies the key
    let mut secrets: Vec<Secret> = serde_json::from_slice(&plain).unwrap_or_default();
    secrets.retain(|s| s.onion != onion);
    secrets.push(Secret {
        onion: onion.to_string(),
        label: label.to_string(),
        privkey_b64: privkey_b64.to_string(),
    });
    write_vault(&salt, key, &secrets)?;
    upsert_index(IndexEntry {
        tier: "passphrase".into(),
        label: label.to_string(),
        onion: None,
        onion_hash: Some(onion_hash(onion)),
    })
}

/// Remove a credential. Passphrase entries require the unlocked key.
pub fn remove(onion: &str, key: Option<&[u8; 32]>) -> Result<()> {
    match tier_of(onion)?.as_deref() {
        Some("os") => {
            let mut secrets = os_secrets().unwrap_or_default();
            secrets.retain(|s| s.onion != onion);
            let blob = dpapi_encrypt(&serde_json::to_vec(&secrets)?)?;
            std::fs::write(os_vault_path()?, blob)?;
        }
        Some("passphrase") => {
            let key = key.ok_or_else(|| anyhow!("unlock required to remove this entry"))?;
            let (salt, nonce, ct) = read_vault()?;
            let plain = aead_decrypt(key, &nonce, &ct)?;
            let mut secrets: Vec<Secret> = serde_json::from_slice(&plain).unwrap_or_default();
            secrets.retain(|s| s.onion != onion);
            write_vault(&salt, key, &secrets)?;
        }
        _ => return Ok(()), // not found, nothing to do
    }
    // Drop from index (match by address or hash).
    let h = onion_hash(onion);
    let entries: Vec<IndexEntry> = list()?
        .into_iter()
        .filter(|e| e.onion.as_deref() != Some(onion) && e.onion_hash.as_deref() != Some(h.as_str()))
        .collect();
    write_index(&entries)
}

// ---------- DPAPI (Windows) -------------------------------------------

#[cfg(windows)]
fn dpapi_encrypt(data: &[u8]) -> Result<Vec<u8>> {
    use std::ptr;
    use winapi::um::dpapi::CryptProtectData;
    use winapi::um::winbase::LocalFree;
    use winapi::um::wincrypt::DATA_BLOB;
    unsafe {
        let mut input = DATA_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = DATA_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &mut input,
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            &mut output,
        );
        if ok == 0 {
            bail!("CryptProtectData failed");
        }
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as *mut _);
        Ok(out)
    }
}

#[cfg(windows)]
fn dpapi_decrypt(data: &[u8]) -> Result<Vec<u8>> {
    use std::ptr;
    use winapi::um::dpapi::CryptUnprotectData;
    use winapi::um::winbase::LocalFree;
    use winapi::um::wincrypt::DATA_BLOB;
    unsafe {
        let mut input = DATA_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = DATA_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &mut input,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            &mut output,
        );
        if ok == 0 {
            bail!("CryptUnprotectData failed");
        }
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as *mut _);
        Ok(out)
    }
}

#[cfg(not(windows))]
fn dpapi_encrypt(_data: &[u8]) -> Result<Vec<u8>> {
    bail!("OS keystore (DPAPI) is only available on Windows")
}

#[cfg(not(windows))]
fn dpapi_decrypt(_data: &[u8]) -> Result<Vec<u8>> {
    bail!("OS keystore (DPAPI) is only available on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onion_hash_is_stable_sha256() {
        // sha256("abc") known vector
        assert_eq!(
            onion_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn aead_roundtrip() {
        let key = [5u8; 32];
        let nonce = [9u8; NONCE_LEN];
        let ct = aead_encrypt(&key, &nonce, b"secret data").unwrap();
        assert_eq!(aead_decrypt(&key, &nonce, &ct).unwrap(), b"secret data");
        let wrong = [6u8; 32];
        assert!(aead_decrypt(&wrong, &nonce, &ct).is_err());
    }

    #[test]
    fn argon2_derivation_is_deterministic() {
        let salt = [1u8; SALT_LEN];
        assert_eq!(derive_key("pw", &salt).unwrap(), derive_key("pw", &salt).unwrap());
        assert_ne!(derive_key("pw", &salt).unwrap(), derive_key("pw", &[2u8; SALT_LEN]).unwrap());
    }

    #[test]
    fn passphrase_roundtrip_and_wrong_pass() {
        // Redirect storage to a temp dir so we never touch the real vault.
        let tmp = std::env::temp_dir().join(format!("or-auth-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("ONIONROUTER_DATA_DIR", &tmp);

        let key = init_passphrase("hunter2").unwrap();
        pp_add(&key, "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd", "blog", "AAAA").unwrap();

        // correct passphrase decrypts and sees the entry
        let (_k, secrets) = unlock("hunter2").unwrap();
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].label, "blog");

        // wrong passphrase fails cleanly
        assert!(unlock("wrong").is_err());

        // index hides the address (hash only) for passphrase tier
        let idx = list().unwrap();
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].tier, "passphrase");
        assert!(idx[0].onion.is_none());
        assert!(idx[0].onion_hash.is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
