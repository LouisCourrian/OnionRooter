//! OnionRouter companion — Firefox Native Messaging host.
//!
//! Lifecycle: Firefox spawns this binary on first `connectNative` from the
//! extension, talks to it over stdin/stdout (length-prefixed JSON), and
//! kills it when the extension disconnects.

mod messaging;
mod proxy;
mod tor_detector;
mod tor_manager;

use anyhow::Result;
use std::sync::Arc;
use tokio::io::{stdin, stdout};
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use messaging::{InboundMessage, OutboundMessage};
use tor_manager::LaunchedTor;

const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(90);

/// Where the SOCKS port we hand to Firefox came from.
enum Backend {
    /// We launched this Tor ourselves; shut it down when asked.
    Owned(LaunchedTor),
    /// Reusing an externally-running Tor; do NOT kill it.
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

/// Shared mutable state across the message loop.
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

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    info!("OnionRouter companion v{} starting", env!("CARGO_PKG_VERSION"));

    let state: Arc<Mutex<State>> = Arc::default();
    let mut stdin = stdin();
    let mut stdout = stdout();

    loop {
        let msg = match messaging::read_message(&mut stdin).await {
            Ok(Some(m)) => m,
            Ok(None) => {
                info!("Firefox closed the channel — shutting down");
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

    // 1. Try to reuse an externally-running Tor (verified via Control Port).
    if let Some(detected) = tor_detector::detect_existing().await {
        info!(
            "reusing external Tor {} on socks={} control={}",
            detected.version, detected.socks_port, detected.control_port
        );
        let port = detected.socks_port;
        state.lock().await.backend = Some(Backend::Reused { socks_port: port });
        return OutboundMessage::Ready { port };
    }

    // 2. Fallback: launch our own.
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
            // We don't own this Tor — just forget about it. Phase 2 §6:
            // "ne pas couper un Tor que l'utilisateur n'a pas lancé".
            warn!("stop requested but Tor was external — leaving it alone");
            OutboundMessage::Stopped
        }
        None => OutboundMessage::Stopped,
    }
}

/// Map low-level errors to short human-readable strings for the popup.
fn friendly_error(e: &anyhow::Error) -> String {
    let chain = format!("{e:#}");
    let lower = chain.to_lowercase();

    if lower.contains("no pinned sha-256") {
        return "internal error: companion binary ships unverified Tor hashes".into();
    }
    if lower.contains("sha-256 mismatch") {
        return "downloaded Tor archive failed integrity check — refusing to run it".into();
    }
    if lower.contains("dns error")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("connect")
    {
        return "could not reach torproject.org — check your internet connection".into();
    }
    if lower.contains("bootstrap") && lower.contains("within") {
        return "Tor took too long to bootstrap — try again or check your network".into();
    }
    if lower.contains("unsupported platform") {
        return "your OS/architecture is not yet supported".into();
    }
    chain
}

fn init_logging() {
    // Native Messaging hijacks stdout, so logs MUST go to stderr / file only.
    let filter = EnvFilter::try_from_env("ONIONROUTER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
