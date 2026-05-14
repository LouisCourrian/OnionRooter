//! Cross-process state file so the system-tray daemon and the Native
//! Messaging shim agree on which SOCKS port Tor is listening on.
//!
//! Why this exists:
//!   The tray daemon (long-lived, started at Windows login) launches Tor
//!   and owns its lifecycle. When Firefox spawns a Native Messaging
//!   instance later, that instance needs to find the tray's Tor without
//!   re-launching its own. The Phase-2 detector probes 9050/9051 and
//!   9150/9151, which catches the tray's Tor in the common case -- but
//!   if those ports were taken at tray startup, Tor ended up on an
//!   OS-allocated port that the detector won't find by guessing.
//!
//! The fix: the tray writes its actual (socks, control) pair to a JSON
//! file under %LOCALAPPDATA%\OnionRouter\runtime\, the NM instance
//! reads it before falling back to probing.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process;

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeState {
    pub socks_port: u16,
    pub control_port: u16,
    /// PID of the tray process holding Tor open. NM instances can use
    /// this to check liveness via OpenProcess (Windows) or kill -0 (Unix).
    pub tray_pid: u32,
    /// Bundle version Tor is running (so a stale file from an older
    /// install can be detected and ignored).
    pub bundle_version: String,
}

fn runtime_dir() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("no local data dir"))?
        .join("OnionRouter")
        .join("runtime");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn state_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("state.json"))
}

/// Persist the live tray state. Best-effort -- failure logs but doesn't
/// abort the tray since this is only an optimisation for the NM bridge.
pub fn write(socks_port: u16, control_port: u16, bundle_version: &str) -> Result<()> {
    let state = RuntimeState {
        socks_port,
        control_port,
        tray_pid: process::id(),
        bundle_version: bundle_version.to_string(),
    };
    let path = state_path()?;
    let json = serde_json::to_vec_pretty(&state)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the most recently published tray state. Returns Ok(None) when
/// the file doesn't exist or is malformed.
pub fn read() -> Option<RuntimeState> {
    let path = state_path().ok()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Best-effort cleanup on tray shutdown.
pub fn clear() {
    if let Ok(path) = state_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Cheap liveness check on the PID recorded by [`write`]. Returns true
/// if a process with that PID exists -- not a guarantee it's still our
/// tray, but enough to filter obvious stale files from a crashed run.
pub fn is_tray_alive(pid: u32) -> bool {
    if pid == 0 || pid == process::id() {
        return false;
    }
    #[cfg(windows)]
    {
        use winapi::shared::minwindef::DWORD;
        use winapi::um::handleapi::CloseHandle;
        use winapi::um::processthreadsapi::OpenProcess;
        use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as DWORD);
            if h.is_null() {
                return false;
            }
            CloseHandle(h);
            true
        }
    }
    #[cfg(not(windows))]
    {
        // On Unix, `kill -0 <pid>` returns 0 iff the process exists.
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}
