//! Detect and verify an externally-running Tor instance via its Control Port.
//!
//! Algorithm:
//!
//! 1. For each `(socks_port, control_port)` in `KNOWN_PAIRS`:
//!    a. Open a TCP connection to `127.0.0.1:control_port`.
//!    b. Send `PROTOCOLINFO 1` to discover supported auth methods.
//!    c. Authenticate (NULL → SAFECOOKIE → COOKIE; HASHEDPASSWORD unsupported).
//!    d. Send `GETINFO version`, parse the response.
//!    e. Compare against `MIN_REUSABLE_TOR_VERSION`.
//!    f. If all checks pass, return the pair.
//! 2. If nothing matches, return `None` → caller launches its own Tor.

use anyhow::{bail, Context, Result};
use rand::RngCore;
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

/// An authenticated control-port connection (read + write halves).
pub(crate) type ControlConn = (
    BufReader<tokio::net::tcp::OwnedReadHalf>,
    tokio::net::tcp::OwnedWriteHalf,
);

/// Connect to a local control port, discover auth methods, and authenticate
/// (NULL → SAFECOOKIE → COOKIE). Returns the authenticated connection, ready
/// for further commands. Shared by detection and client-auth injection.
pub(crate) async fn connect_and_auth(control_port: u16) -> Result<ControlConn> {
    let stream = TcpStream::connect(("127.0.0.1", control_port))
        .await
        .context("tcp connect")?;
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);

    write.write_all(b"PROTOCOLINFO 1\r\n").await?;
    let mut auth = AuthInfo::default();
    if !read_response(&mut reader, |line| auth.parse_line(line)).await? {
        bail!("PROTOCOLINFO refused");
    }
    if !authenticate(&mut reader, &mut write, &auth).await? {
        bail!("control port authentication failed");
    }
    Ok((reader, write))
}

/// Send one control command (CRLF appended) and return whether the final
/// reply was a 2xx success.
pub(crate) async fn send_command(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    cmd: &str,
) -> Result<bool> {
    write.write_all(cmd.as_bytes()).await?;
    write.write_all(b"\r\n").await?;
    await_ok(reader).await
}

async fn verify_pair(socks_port: u16, control_port: u16) -> Result<Option<DetectedTor>> {
    let (mut reader, mut write) = match connect_and_auth(control_port).await {
        Ok(conn) => conn,
        Err(e) => {
            debug!("control {control_port}: {e:#}");
            return Ok(None);
        }
    };

    // Ask Tor for its version.
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
    // NULL auth: no auth configured, just send AUTHENTICATE.
    if auth.null {
        write.write_all(b"AUTHENTICATE\r\n").await?;
        return await_ok(reader).await;
    }

    // SAFECOOKIE preferred over raw COOKIE: it proves the cookie file to the
    // server without sending it in the clear, and it's what Tor Browser uses.
    if auth.safe_cookie {
        if let Some(path) = &auth.cookie_file {
            match safecookie_auth(reader, write, path).await {
                Ok(ok) => return Ok(ok),
                // A failed challenge leaves the connection in an unknown state,
                // so don't try another method on it -- let the caller fall back.
                Err(e) => {
                    debug!("SAFECOOKIE auth failed: {e:#}");
                    return Ok(false);
                }
            }
        }
    }

    // Raw COOKIE: read the 32-byte cookie file and send it hex-encoded.
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

    // HASHEDPASSWORD needs a password we don't have. Refuse.
    Ok(false)
}

/// SAFECOOKIE handshake (control-spec §3.24): prove knowledge of the cookie
/// file via an HMAC challenge/response, without sending the cookie in clear.
async fn safecookie_auth(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    cookie_path: &std::path::Path,
) -> Result<bool> {
    let cookie = tokio::fs::read(cookie_path)
        .await
        .with_context(|| format!("reading cookie file {}", cookie_path.display()))?;
    if cookie.len() != 32 {
        bail!("unexpected control cookie length: {} (want 32)", cookie.len());
    }

    // 1. Send a 32-byte client nonce.
    let mut client_nonce = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut client_nonce);
    let cmd = format!("AUTHCHALLENGE SAFECOOKIE {}\r\n", hex::encode(client_nonce));
    write.write_all(cmd.as_bytes()).await?;

    // 2. Read SERVERHASH / SERVERNONCE from the AUTHCHALLENGE reply.
    let mut server_hash_hex: Option<String> = None;
    let mut server_nonce_hex: Option<String> = None;
    let ok = read_response(reader, |line| {
        let rest = line
            .strip_prefix("250 AUTHCHALLENGE ")
            .or_else(|| line.strip_prefix("250-AUTHCHALLENGE "));
        if let Some(rest) = rest {
            for tok in rest.split_whitespace() {
                if let Some(v) = tok.strip_prefix("SERVERHASH=") {
                    server_hash_hex = Some(v.to_string());
                } else if let Some(v) = tok.strip_prefix("SERVERNONCE=") {
                    server_nonce_hex = Some(v.to_string());
                }
            }
        }
    })
    .await?;
    if !ok {
        return Ok(false);
    }

    let server_hash =
        hex::decode(server_hash_hex.context("AUTHCHALLENGE without SERVERHASH")?)
            .context("decoding SERVERHASH")?;
    let server_nonce =
        hex::decode(server_nonce_hex.context("AUTHCHALLENGE without SERVERNONCE")?)
            .context("decoding SERVERNONCE")?;

    // 3. message = CookieString | ClientNonce | ServerNonce
    let mut msg = Vec::with_capacity(cookie.len() + client_nonce.len() + server_nonce.len());
    msg.extend_from_slice(&cookie);
    msg.extend_from_slice(&client_nonce);
    msg.extend_from_slice(&server_nonce);

    // 4. Verify the server proved knowledge of the cookie (authenticates Tor
    //    to us). Constant-time compare.
    let expected = hmac_sha256(
        b"Tor safe cookie authentication server-to-controller hash",
        &msg,
    );
    if !ct_eq(&expected, &server_hash) {
        bail!("SERVERHASH mismatch -- control port did not prove the cookie");
    }

    // 5. Send our proof and expect 250 OK.
    let client_hash = hmac_sha256(
        b"Tor safe cookie authentication controller-to-server hash",
        &msg,
    );
    let cmd = format!("AUTHENTICATE {}\r\n", hex::encode(client_hash));
    write.write_all(cmd.as_bytes()).await?;
    await_ok(reader).await
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time byte-slice equality.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

    #[test]
    fn hmac_sha256_rfc4231_case2() {
        // RFC 4231 test case 2 -- validates our HMAC wiring used by SAFECOOKIE.
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn ct_eq_compares() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
