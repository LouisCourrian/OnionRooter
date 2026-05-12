//! SOCKS5 port allocation.
//!
//! The companion does not actually run a SOCKS5 server — Tor does. This
//! module only picks ports and tracks which (SOCKS, control) pair the
//! current Tor instance is exposing, so the extension can be told where to
//! send traffic.

use anyhow::{Context, Result};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

/// Default ports we prefer when no external Tor is reused.
pub const DEFAULT_SOCKS_PORT: u16 = 9050;
pub const DEFAULT_CONTROL_PORT: u16 = 9051;

/// Try to bind the preferred port; if taken, ask the OS for any free port.
pub fn pick_port(preferred: u16) -> Result<u16> {
    if let Ok(listener) = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, preferred)) {
        let port = listener.local_addr()?.port();
        drop(listener);
        return Ok(port);
    }
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .context("binding any free port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Allocate (SOCKS, control) ports, preferring the standard pair.
pub fn allocate_pair() -> Result<(u16, u16)> {
    let socks = pick_port(DEFAULT_SOCKS_PORT)?;
    let control = pick_port(DEFAULT_CONTROL_PORT)?;
    Ok((socks, control))
}
