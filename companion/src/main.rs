//! OnionRouter companion -- Firefox Native Messaging host + Windows
//! system-tray daemon, same binary, two modes.
//!
//! Default (no args)         -> Native Messaging mode. Firefox spawns us,
//!                              talks over stdin/stdout, kills us when the
//!                              extension disconnects.
//! `--tray` (Windows only)   -> Tray daemon. Long-lived, owns Tor's
//!                              lifecycle, drawn in the notification area.
//!                              Started at user login via a Run registry
//!                              key written by the installer.
//!
//! Subsystem note: on Windows release builds we mark the binary as the
//! "windows" subsystem so no console window pops up when Windows spawns
//! us (either via the Run key or via Firefox's Native Messaging). Debug
//! builds keep the default console subsystem so `cargo run` from a
//! terminal still shows stderr live.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod messaging;
mod proxy;
mod runtime;
mod tor_detector;
mod tor_manager;
mod tor_update;

#[cfg(windows)]
mod tray;

use anyhow::Result;
use std::sync::Arc;
use tokio::io::{stdin, stdout};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use messaging::{InboundMessage, OutboundMessage};
use tor_manager::LaunchedTor;

const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(90);
const NATIVE_MESSAGING_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Where the SOCKS port we hand to Firefox came from.
enum Backend {
    /// We launched this Tor ourselves; shut it down when the NM session ends.
    Owned(LaunchedTor),
    /// Reusing a Tor owned by another process (Tor Browser, system Tor,
    /// our own tray daemon...). Do NOT kill it on Stop.
    Reused { socks_port: u16 },
}

impl Backend {
    fn socks_port(&self) -> u16 {
        match self {
            Backend::Owned(t) => t.socks_port,
            Backend::Reused { socks_port } => *socks_port,
        }
    }
}

/// Diagnostic metadata captured when a backend becomes active. Kept
/// alongside the backend so the extension's diagnostic page can report
/// where Tor came from, which ports it uses, and its version.
#[derive(Clone)]
struct DiagInfo {
    /// "owned" | "tray" | "external".
    source: &'static str,
    socks_port: u16,
    control_port: u16,
    /// Tor daemon version, only known for reused external instances.
    tor_version: Option<String>,
}

#[derive(Default)]
struct State {
    backend: Option<Backend>,
    info: Option<DiagInfo>,
}

impl State {
    fn snapshot(&self) -> OutboundMessage {
        match &self.backend {
            Some(b) => OutboundMessage::Ready {
                port: b.socks_port(),
            },
            None => OutboundMessage::Stopped,
        }
    }

    fn diagnostic(&self) -> OutboundMessage {
        let info = self.info.as_ref();
        OutboundMessage::Diagnostic {
            running: self.backend.is_some(),
            source: info.map(effective_source),
            socks_port: info.map(|i| i.socks_port),
            control_port: info.map(|i| i.control_port),
            tor_version: info.and_then(|i| i.tor_version.clone()),
            // Report the version actually installed on disk (what auto-update
            // fetched), falling back to the pinned baseline.
            bundle_version: tor_manager::installed_version()
                .unwrap_or_else(|| tor_manager::BUNDLE_VERSION.to_string()),
            companion_version: env!("CARGO_PKG_VERSION").to_string(),
            platform: tor_manager::host_platform(),
            data_dir: tor_manager::data_dir().map(|p| p.display().to_string()),
        }
    }
}

/// At connect time a Native Messaging instance may probe the tray's own Tor on
/// 9050 before the tray has written its runtime file, labelling it "external".
/// By diagnostic-query time the file exists, so relabel to "tray" when a live
/// tray published the same SOCKS port.
fn effective_source(i: &DiagInfo) -> String {
    if i.source == "external" {
        if let Some(rt) = runtime::read() {
            if runtime::is_tray_alive(rt.tray_pid) && rt.socks_port == i.socks_port {
                return "tray".to_string();
            }
        }
    }
    i.source.to_string()
}

fn main() -> Result<()> {
    init_logging();

    // Mode dispatch based on argv. Keep this BEFORE the tokio runtime
    // because the tray needs its own runtime setup (custom shutdown).
    let args: Vec<String> = std::env::args().collect();

    #[cfg(windows)]
    if args.iter().any(|a| a == "--tray") {
        info!(
            "OnionRouter companion v{} starting in tray mode",
            env!("CARGO_PKG_VERSION")
        );
        return tray::run();
    }

    info!(
        "OnionRouter companion v{} starting in native-messaging mode",
        env!("CARGO_PKG_VERSION")
    );

    // Native Messaging mode -- async event loop.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_native_messaging())
}

async fn run_native_messaging() -> Result<()> {
    let state: Arc<Mutex<State>> = Arc::default();
    let mut stdin = stdin();
    let mut stdout = stdout();

    loop {
        let msg = match messaging::read_message(&mut stdin).await {
            Ok(Some(m)) => m,
            Ok(None) => {
                info!("Firefox closed the channel -- shutting down");
                break;
            }
            Err(e) => {
                error!("inbound read failed: {e:#}");
                let _ = messaging::write_message(
                    &mut stdout,
                    &OutboundMessage::Error {
                        message: format!("protocol error: {e}"),
                    },
                )
                .await;
                break;
            }
        };

        let reply = handle(&state, msg).await;
        if let Err(e) = messaging::write_message(&mut stdout, &reply).await {
            error!("failed to send reply: {e:#}");
            break;
        }
    }

    // Make sure we don't leave an orphan Tor process behind.
    let mut state = state.lock().await;
    if let Some(Backend::Owned(mut tor)) = state.backend.take() {
        info!("stopping owned Tor before exit");
        tor_manager::shutdown(&mut tor).await;
    }
    Ok(())
}

async fn handle(state: &Arc<Mutex<State>>, msg: InboundMessage) -> OutboundMessage {
    match msg {
        InboundMessage::Ping => OutboundMessage::Pong,
        InboundMessage::Status => state.lock().await.snapshot(),
        InboundMessage::Diagnostic => state.lock().await.diagnostic(),
        InboundMessage::Stop => stop(state).await,
        InboundMessage::Start => start(state).await,
    }
}

async fn start(state: &Arc<Mutex<State>>) -> OutboundMessage {
    {
        let s = state.lock().await;
        if let Some(b) = &s.backend {
            return OutboundMessage::Ready {
                port: b.socks_port(),
            };
        }
    }

    // 1. Tray-published runtime file -- if the OnionRouter tray daemon
    //    is running, it tells us exactly which port to use, no probing
    //    needed. Best-effort: ignored if the file is stale.
    if let Some(rt) = runtime::read() {
        if runtime::is_tray_alive(rt.tray_pid) {
            if tcp_alive(rt.socks_port).await {
                info!(
                    "found tray-published Tor on socks={} control={} (pid {})",
                    rt.socks_port, rt.control_port, rt.tray_pid
                );
                {
                    let mut s = state.lock().await;
                    s.backend = Some(Backend::Reused {
                        socks_port: rt.socks_port,
                    });
                    s.info = Some(DiagInfo {
                        source: "tray",
                        socks_port: rt.socks_port,
                        control_port: rt.control_port,
                        tor_version: None,
                    });
                }
                return OutboundMessage::Ready {
                    port: rt.socks_port,
                };
            } else {
                warn!("tray runtime file points at port {} but nothing is listening", rt.socks_port);
            }
        } else {
            warn!(
                "tray runtime file references dead pid {} -- ignoring",
                rt.tray_pid
            );
        }
    }

    // 2. Probe the well-known pairs (system Tor on 9050/9051, Tor Browser
    //    on 9150/9151) with full Control Port verification. Phase 2.
    if let Some(detected) = tor_detector::detect_existing().await {
        info!(
            "reusing external Tor {} on socks={} control={}",
            detected.version, detected.socks_port, detected.control_port
        );
        let port = detected.socks_port;
        {
            let mut s = state.lock().await;
            s.backend = Some(Backend::Reused { socks_port: port });
            s.info = Some(DiagInfo {
                source: "external",
                socks_port: detected.socks_port,
                control_port: detected.control_port,
                tor_version: Some(detected.version.to_string()),
            });
        }
        return OutboundMessage::Ready { port };
    }

    // 3. Fallback: launch our own.
    match launch_owned().await {
        Ok(launched) => {
            let port = launched.socks_port;
            let control_port = launched.control_port;
            {
                let mut s = state.lock().await;
                s.backend = Some(Backend::Owned(launched));
                s.info = Some(DiagInfo {
                    source: "owned",
                    socks_port: port,
                    control_port,
                    tor_version: None,
                });
            }
            OutboundMessage::Ready { port }
        }
        Err(e) => {
            error!("failed to start Tor: {e:#}");
            OutboundMessage::Error {
                message: friendly_error(&e),
            }
        }
    }
}

async fn tcp_alive(port: u16) -> bool {
    matches!(
        tokio::time::timeout(NATIVE_MESSAGING_PROBE_TIMEOUT, TcpStream::connect(("127.0.0.1", port))).await,
        Ok(Ok(_))
    )
}

async fn launch_owned() -> Result<LaunchedTor> {
    let binary = tor_manager::ensure_binary().await?;
    let (socks, control) = proxy::allocate_pair()?;
    tor_manager::launch(&binary, socks, control, BOOTSTRAP_TIMEOUT).await
}

async fn stop(state: &Arc<Mutex<State>>) -> OutboundMessage {
    let mut s = state.lock().await;
    s.info = None;
    match s.backend.take() {
        Some(Backend::Owned(mut tor)) => {
            tor_manager::shutdown(&mut tor).await;
            OutboundMessage::Stopped
        }
        Some(Backend::Reused { .. }) => {
            warn!("stop requested but Tor was external -- leaving it alone");
            OutboundMessage::Stopped
        }
        None => OutboundMessage::Stopped,
    }
}

fn friendly_error(e: &anyhow::Error) -> String {
    let chain = format!("{e:#}");
    let lower = chain.to_lowercase();

    if lower.contains("no pinned sha-256") {
        return "internal error: companion binary ships unverified Tor hashes".into();
    }
    if lower.contains("sha-256 mismatch") {
        return "downloaded Tor archive failed integrity check -- refusing to run it".into();
    }
    if lower.contains("dns error")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("connect")
    {
        return "could not reach torproject.org -- check your internet connection".into();
    }
    if lower.contains("bootstrap") && lower.contains("within") {
        return "Tor took too long to bootstrap -- try again or check your network".into();
    }
    if lower.contains("unsupported platform") {
        return "your OS/architecture is not yet supported".into();
    }
    chain
}

/// Where the companion writes its log file. Exposed so the tray menu
/// can open the folder in Explorer ("Open logs folder").
pub fn log_dir_path() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("OnionRouter").join("logs"))
}

fn init_logging() {
    let filter =
        EnvFilter::try_from_env("ONIONROUTER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    // Always log to file. Without the "windows" subsystem there is no
    // console attached when Windows spawns us via the Run key or via
    // Firefox's Native Messaging, so stderr would just sink. A real
    // file gives the user something to share when reporting bugs, and
    // the tray menu has an "Open logs folder" item to surface it.
    if let Some(dir) = log_dir_path() {
        if std::fs::create_dir_all(&dir).is_ok() {
            let appender = tracing_appender::rolling::never(&dir, "companion.log");
            if tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(appender)
                .with_ansi(false)
                .try_init()
                .is_ok()
            {
                return;
            }
        }
    }

    // Fallback if the log dir is unwritable (read-only home, sandbox...).
    let fallback_filter =
        EnvFilter::try_from_env("ONIONROUTER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(fallback_filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
