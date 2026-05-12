//! Detect and verify an externally-running Tor instance via its Control Port.
//!
//! Phase 1: scaffolding + TCP probe only.
//! Phase 2 will complete the Control Port handshake (AUTHENTICATE / GETINFO
//! version) and the minimum-version check defined in
//! `tor_manager::MIN_REUSABLE_TOR_VERSION`.

use anyhow::Result;
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};
use tracing::debug;

/// Candidate (SOCKS port, Control port) pairs to probe, in priority order.
pub const KNOWN_PAIRS: &[(u16, u16)] = &[
    (9050, 9051), // system Tor service
    (9150, 9151), // Tor Browser
];

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Result of a successful detection.
#[derive(Debug, Clone, Copy)]
pub struct DetectedTor {
    pub socks_port: u16,
    pub control_port: u16,
}

/// Scan known pairs and return the first one that responds on the Control Port.
///
/// **Phase 1 limitation**: only verifies that *something* listens on the
/// control port. Returning `Some(_)` here does NOT yet imply the listener is
/// actually Tor or that its version is acceptable — Phase 2 will add the
/// AUTHENTICATE + GETINFO version handshake before reuse is allowed.
///
/// Until Phase 2 lands, callers SHOULD treat this as "hint only" and prefer
/// launching their own Tor process.
pub async fn detect_existing() -> Option<DetectedTor> {
    for &(socks, control) in KNOWN_PAIRS {
        if probe_tcp(control).await {
            debug!("control port {control} is reachable (unverified)");
            return Some(DetectedTor {
                socks_port: socks,
                control_port: control,
            });
        }
    }
    None
}

async fn probe_tcp(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    matches!(
        timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

/// Phase-2 placeholder: full Control Port verification.
#[allow(dead_code)]
pub async fn verify_is_tor(_control_port: u16) -> Result<bool> {
    // TODO Phase 2:
    //   1. connect to 127.0.0.1:control_port
    //   2. write `AUTHENTICATE ""\r\n`, expect `250 OK`
    //   3. write `GETINFO version\r\n`, expect `250-version=...`
    //   4. parse version, compare to MIN_REUSABLE_TOR_VERSION
    //   5. write `QUIT\r\n`
    Ok(false)
}
