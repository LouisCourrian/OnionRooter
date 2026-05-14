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

mod messaging;
mod proxy;
mod runtime;
mod tor_detector;
mod tor_manager;

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

#[derive(Default)]
struct State {
    backend: Option<Backend>,
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
                state.lock().await.backend = Some(Backend::Reused {
                    socks_port: rt.socks_port,
                });
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
        state.lock().await.backend = Some(Backend::Reused { socks_port: port });
        return OutboundMessage::Ready { port };
    }

    // 3. Fallback: launch our own.
    match launch_owned().await {
        Ok(launched) => {
            let port = launched.socks_port;
            state.lock().await.backend = Some(Backend::Owned(launched));
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

fn init_logging() {
    let filter =
        EnvFilter::try_from_env("ONIONROUTER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    // In Native Messaging mode stdout is the protocol channel -- logs
    // MUST go to stderr. The tray daemon could log to a file but for
    // now stderr is fine too (no console attached when launched via
    // the Run key, so logs are silently dropped; debugging can be done
    // by launching from a terminal manually).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
