//! Windows system tray daemon.
//!
//! Long-lived process started at user login (via the Run registry key
//! the installer adds). Launches Tor once on startup, keeps it alive
//! across Firefox sessions, and exposes a small tray menu so the user
//! can quit it or open the project's GitHub page.
//!
//! The Native Messaging instance Firefox spawns later finds this Tor
//! through the runtime state file (see `runtime.rs`) and reuses it
//! instead of launching its own.

#![cfg(windows)]

use anyhow::{Context, Result};
use std::sync::mpsc;
use tokio::time::Duration;
use tracing::{error, info, warn};
use tray_item::{IconSource, TrayItem};

use crate::tor_manager::{self, BUNDLE_VERSION, LaunchedTor};
use crate::{proxy, runtime, tor_detector};

const GITHUB_URL: &str = "https://github.com/LouisCourrian/OnionRooter";

/// Tray menu messages dispatched from the GUI callback thread back to
/// the main thread waiting on the mpsc channel.
enum TrayEvent {
    OpenGitHub,
    Quit,
}

/// Which Tor are we using.
enum Backend {
    Owned(LaunchedTor),
    Reused { socks_port: u16, control_port: u16 },
}

impl Backend {
    fn ports(&self) -> (u16, u16) {
        match self {
            Backend::Owned(t) => (t.socks_port, t.control_port),
            Backend::Reused {
                socks_port,
                control_port,
            } => (*socks_port, *control_port),
        }
    }
}

/// Start the daemon. Blocks until the user picks "Quit" or the process
/// is killed externally.
pub fn run() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("creating tokio runtime")?;

    // 1. Establish Tor -- detect existing first, fall back to launching ours.
    let backend = rt.block_on(establish_tor())?;
    let (socks_port, control_port) = backend.ports();

    // 2. Publish the port so Firefox-launched NM instances can find it.
    if let Err(e) = runtime::write(socks_port, control_port, BUNDLE_VERSION) {
        warn!("could not write runtime state file: {e:#}");
    }

    info!(
        "tray daemon ready -- Tor SOCKS on 127.0.0.1:{socks_port} (control {control_port})"
    );

    // 3. Set up the system tray icon + menu.
    // The "MAINICON" resource is baked into the .exe by build.rs via
    // winres -- it's a 16+32px purple-onion .ico generated procedurally
    // at build time.
    let (tx, rx) = mpsc::channel::<TrayEvent>();
    let mut tray = TrayItem::new(
        &format!("OnionRouter -- Tor on 127.0.0.1:{socks_port}"),
        IconSource::Resource("MAINICON"),
    )
    .context("creating tray icon")?;

    tray.add_label(&format!("SOCKS port: {socks_port}"))
        .context("adding status label")?;

    let tx_gh = tx.clone();
    tray.add_menu_item("Open project on GitHub", move || {
        let _ = tx_gh.send(TrayEvent::OpenGitHub);
    })
    .context("adding GitHub menu item")?;

    let tx_q = tx.clone();
    tray.add_menu_item("Quit OnionRouter", move || {
        let _ = tx_q.send(TrayEvent::Quit);
    })
    .context("adding Quit menu item")?;

    // 4. Hold the backend alive in this scope until shutdown.
    let mut backend = Some(backend);

    // 5. Pump menu events until Quit.
    loop {
        match rx.recv() {
            Ok(TrayEvent::OpenGitHub) => {
                if let Err(e) = webbrowser::open(GITHUB_URL) {
                    warn!("failed to open browser: {e}");
                }
            }
            Ok(TrayEvent::Quit) => {
                info!("Quit selected from tray menu");
                break;
            }
            Err(e) => {
                error!("tray channel closed unexpectedly: {e}");
                break;
            }
        }
    }

    // 6. Shutdown inside the tokio runtime so kill_on_drop fires with
    //    the runtime still active. Dropping Tor outside of any tokio
    //    context can race with the child reaper.
    runtime::clear();
    rt.block_on(async {
        if let Some(b) = backend.take() {
            if let Backend::Owned(mut t) = b {
                tor_manager::shutdown(&mut t).await;
            }
        }
        // Tiny pause to let kill_on_drop side effects settle on Windows.
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    // Drop the tray AFTER we've cleaned up Tor so the icon disappears
    // only once shutdown is actually complete.
    drop(tray);

    info!("tray daemon exiting");
    Ok(())
}

/// Detect-or-launch Tor. Mirrors the strategy used by the Native
/// Messaging mode in `main.rs` so the behaviour is consistent.
async fn establish_tor() -> Result<Backend> {
    if let Some(detected) = tor_detector::detect_existing().await {
        info!(
            "reusing external Tor {} on socks={} control={}",
            detected.version, detected.socks_port, detected.control_port
        );
        return Ok(Backend::Reused {
            socks_port: detected.socks_port,
            control_port: detected.control_port,
        });
    }
    let binary = tor_manager::ensure_binary().await?;
    let (socks, control) = proxy::allocate_pair()?;
    let launched = tor_manager::launch(&binary, socks, control, Duration::from_secs(90)).await?;
    Ok(Backend::Owned(launched))
}

// The procedural icon generator lives in build.rs and is baked into
// the .exe as a Windows resource named "MAINICON" via winres.
