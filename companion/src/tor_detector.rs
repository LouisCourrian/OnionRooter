//! Detect and verify an externally-running Tor instance via its Control Port.
//!
//! Algorithm (per the cahier des charges §4.3):
//!
//! 1. For each `(socks_port, control_port)` in `KNOWN_PAIRS`:
//!    a. Open a TCP connection to `127.0.0.1:control_port`.
//!    b. Send `PROTOCOLINFO 1` to discover supported auth methods.
//!    c. Authenticate (NULL → COOKIE → give up if only SAFECOOKIE/HASHEDPASS).
//!    d. Send `GETINFO version`, parse the response.
//!    e. Compare against `MIN_REUSABLE_TOR_VERSION`.
//!    f. If all checks pass, return the pair.
//! 2. If nothing matches, return `None` → caller launches its own Tor.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::Duration as StdDuration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::tor_manager::MIN_REUSABLE_TOR_VERSION;

/// Candidate (SOCKS port, Control port) pairs to probe, in priority order.
pub const KNOWN_PAIRS: &[(u16, u16)] = &[
    (9050, 9051), // system Tor service
    (9150, 9151), // Tor Browser
];

const HANDSHAKE_TIMEOUT: StdDuration = StdDuration::from_secs(3);
const READ_TIMEOUT: StdDuration = StdDuration::from_secs(2);

/// Result of a successful detection.
#[derive(Debug, Clone, Copy)]
pub struct DetectedTor {
    pub socks_port: u16,
    pub control_port: u16,
    pub version: TorVersion,
}

/// Tor binary version, parsed from `GETINFO version`.
///
/// Tor reports versions like `0.4.8.12`, sometimes followed by a suffix
/// (`-alpha-dev`) or git annotation (` (git-abc)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TorVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub micro: u32,
}

impl TorVersion {
    /// Parse the version field from a `version=` line or a bare version string.
    ///
    /// Accepts: `0.4.8.12`, `0.4.8.12-alpha`, `0.4.8.12 (git-abc)`.
    pub fn parse(raw: &str) -> Option<Self> {
        // Strip anything after the first whitespace, dash, or paren.
        let core: String = raw
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let mut parts = core.split('.').filter_map(|p| p.parse::<u32>().ok());
        let major = parts.next()?;
        let minor = parts.next()?;
        let patch = parts.next()?;
        let micro = parts.next().unwrap_or(0);
        Some(Self {
            major,
            minor,
            patch,
            micro,
        })
    }

    pub fn as_tuple(self) -> (u32, u32, u32, u32) {
        (self.major, self.minor, self.patch, self.micro)
    }

    pub fn is_at_least(self, other: (u32, u32, u32, u32)) -> bool {
        self.as_tuple() >= other
    }
}

impl std::fmt::Display for TorVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.patch, self.micro
        )
    }
}

/// Auth methods advertised by `PROTOCOLINFO`.
#[derive(Debug, Default)]
struct AuthInfo {
    null: bool,
    cookie: bool,
    safe_cookie: bool,
    hashed_password: bool,
    cookie_file: Option<PathBuf>,
}

impl AuthInfo {
    fn parse_line(&mut self, line: &str) {
        // Example:
        //   250-AUTH METHODS=COOKIE,SAFECOOKIE COOKIEFILE="/run/tor/control.authcookie"
        let Some(rest) = line.strip_prefix("250-AUTH ").or_else(|| line.strip_prefix("250 AUTH "))
        else {
            return;
        };
        for token in rest.split_whitespace() {
            if let Some(methods) = token.strip_prefix("METHODS=") {
                for m in methods.split(',') {
                    match m {
                        "NULL" => self.null = true,
                        "COOKIE" => self.cookie = true,
                        "SAFECOOKIE" => self.safe_cookie = true,
                        "HASHEDPASSWORD" => self.hashed_password = true,
                        _ => {}
                    }
                }
            } else if let Some(path) = token.strip_prefix("COOKIEFILE=") {
                // Strip surrounding quotes and unescape Tor-style backslash
                // escapes (Tor escapes `"` and `\` inside quoted strings).
                let unquoted = path.trim_matches('"');
                let unescaped = unquoted.replace("\\\\", "\\").replace("\\\"", "\"");
                self.cookie_file = Some(PathBuf::from(unescaped));
            }
        }
    }
}

/// Scan known pairs and return the first reusable Tor instance.
pub async fn detect_existing() -> Option<DetectedTor> {
    for &(socks, control) in KNOWN_PAIRS {
        match timeout(HANDSHAKE_TIMEOUT, verify_pair(socks, control)).await {
            Ok(Ok(Some(detected))) => return Some(detected),
            Ok(Ok(None)) => debug!("control port {control}: rejected (auth/version)"),
            Ok(Err(e)) => debug!("control port {control}: probe failed: {e:#}"),
            Err(_) => debug!("control port {control}: probe timed out"),
        }
    }
    None
}

async fn verify_pair(socks_port: u16, control_port: u16) -> Result<Option<DetectedTor>> {
    let stream = TcpStream::connect(("127.0.0.1", control_port))
        .await
        .context("tcp connect")?;
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);

    // 1. Discover auth methods.
    write.write_all(b"PROTOCOLINFO 1\r\n").await?;
    let mut auth = AuthInfo::default();
    if !read_response(&mut reader, |line| auth.parse_line(line)).await? {
        debug!("PROTOCOLINFO refused");
        return Ok(None);
    }

    // 2. Authenticate, preferring the simplest method.
    if !authenticate(&mut reader, &mut write, &auth).await? {
        debug!("authentication failed (advertised: {auth:?})");
        return Ok(None);
    }

    // 3. Ask Tor for its version.
    let Some(version) = ask_version(&mut reader, &mut write).await? else {
        return Ok(None);
    };

    // 4. Enforce minimum version.
    if !version.is_at_least(MIN_REUSABLE_TOR_VERSION) {
        warn!(
            "ignoring external Tor {version}: below minimum {:?}",
            MIN_REUSABLE_TOR_VERSION
        );
        let _ = write.write_all(b"QUIT\r\n").await;
        return Ok(None);
    }

    // 5. Be polite.
    let _ = write.write_all(b"QUIT\r\n").await;

    info!("reusing external Tor {version} on socks={socks_port} control={control_port}");
    Ok(Some(DetectedTor {
        socks_port,
        control_port,
        version,
    }))
}

async fn authenticate(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    auth: &AuthInfo,
) -> Result<bool> {
    // NULL auth: no arg.
    if auth.null {
        write.write_all(b"AUTHENTICATE\r\n").await?;
        return await_ok(reader).await;
    }

    // Cookie auth: read 32-byte cookie file, hex-encode.
    if auth.cookie {
        if let Some(path) = &auth.cookie_file {
            match tokio::fs::read(path).await {
                Ok(bytes) => {
                    let hex_cookie = hex::encode(&bytes);
                    let cmd = format!("AUTHENTICATE {hex_cookie}\r\n");
                    write.write_all(cmd.as_bytes()).await?;
                    return await_ok(reader).await;
                }
                Err(e) => {
                    debug!("cookie file {} unreadable: {e}", path.display());
                }
            }
        }
    }

    // SAFECOOKIE / HASHEDPASSWORD: too complex for MVP. Phase 4 may add
    // SAFECOOKIE (HMAC challenge-response). For now, refuse rather than
    // reuse a Tor we can't authenticate to.
    Ok(false)
}

async fn ask_version(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write: &mut tokio::net::tcp::OwnedWriteHalf,
) -> Result<Option<TorVersion>> {
    write.write_all(b"GETINFO version\r\n").await?;
    let mut version: Option<TorVersion> = None;
    let ok = read_response(reader, |line| {
        // line examples:
        //   "250-version=Tor 0.4.8.12 (git-abc)"
        //   "250-version=0.4.8.12"
        //   "250 OK"
        if let Some(rest) = line
            .strip_prefix("250-version=")
            .or_else(|| line.strip_prefix("250 version="))
        {
            let raw = rest.strip_prefix("Tor ").unwrap_or(rest);
            version = TorVersion::parse(raw);
        }
    })
    .await?;
    if !ok {
        return Ok(None);
    }
    Ok(version)
}

/// Read until a final-line response (status code followed by space). Returns
/// true if the final status was 2xx. Calls `on_line` for every line including
/// the final one. Bails on read errors or socket close.
async fn read_response<F>(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    mut on_line: F,
) -> Result<bool>
where
    F: FnMut(&str),
{
    loop {
        let mut line = String::new();
        let bytes = timeout(READ_TIMEOUT, reader.read_line(&mut line))
            .await
            .context("control read timed out")??;
        if bytes == 0 {
            bail!("control connection closed mid-response");
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        on_line(trimmed);

        if trimmed.len() < 4 {
            continue;
        }
        let status: &str = &trimmed[..3];
        let sep = trimmed.as_bytes()[3];
        if sep == b' ' {
            // final line
            return Ok(status.starts_with('2'));
        }
        // sep == b'-' or b'+' → continuation
    }
}

async fn await_ok(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Result<bool> {
    read_response(reader, |_| {}).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_version() {
        let v = TorVersion::parse("0.4.8.12").unwrap();
        assert_eq!(v.as_tuple(), (0, 4, 8, 12));
    }

    #[test]
    fn parses_version_with_git_suffix() {
        let v = TorVersion::parse("0.4.8.12 (git-abc)").unwrap();
        assert_eq!(v.as_tuple(), (0, 4, 8, 12));
    }

    #[test]
    fn parses_version_with_alpha_suffix() {
        let v = TorVersion::parse("0.4.8.13-alpha-dev").unwrap();
        assert_eq!(v.as_tuple(), (0, 4, 8, 13));
    }

    #[test]
    fn parses_three_component_version() {
        // Some old versions report 3 components — micro defaults to 0.
        let v = TorVersion::parse("0.4.8").unwrap();
        assert_eq!(v.as_tuple(), (0, 4, 8, 0));
    }

    #[test]
    fn rejects_garbage() {
        assert!(TorVersion::parse("").is_none());
        assert!(TorVersion::parse("not a version").is_none());
        assert!(TorVersion::parse("0").is_none());
    }

    #[test]
    fn version_comparison() {
        let v = TorVersion::parse("0.4.8.12").unwrap();
        assert!(v.is_at_least((0, 4, 7, 0)));
        assert!(v.is_at_least((0, 4, 8, 12)));
        assert!(!v.is_at_least((0, 4, 8, 13)));
        assert!(!v.is_at_least((0, 5, 0, 0)));
    }

    #[test]
    fn parses_auth_null() {
        let mut a = AuthInfo::default();
        a.parse_line("250-AUTH METHODS=NULL");
        assert!(a.null);
        assert!(!a.cookie);
    }

    #[test]
    fn parses_auth_cookie_with_file() {
        let mut a = AuthInfo::default();
        a.parse_line(r#"250-AUTH METHODS=COOKIE,SAFECOOKIE COOKIEFILE="/run/tor/control.authcookie""#);
        assert!(a.cookie);
        assert!(a.safe_cookie);
        assert_eq!(
            a.cookie_file.as_deref().map(|p| p.to_string_lossy().into_owned()),
            Some("/run/tor/control.authcookie".to_string())
        );
    }

    #[test]
    fn parses_auth_hashed_password() {
        let mut a = AuthInfo::default();
        a.parse_line("250-AUTH METHODS=HASHEDPASSWORD");
        assert!(a.hashed_password);
        assert!(!a.null);
    }
}
