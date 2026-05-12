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

/// Shared mutable state across the message loop.
#[derive(Default)]
struct State {
    tor: Option<LaunchedTor>,
}

impl State {
    fn snapshot(&self) -> OutboundMessage {
        match &self.tor {
            Some(t) => OutboundMessage::Ready {
                port: t.socks_port,
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
    if let Some(mut tor) = state.tor.take() {
        info!("stopping Tor before exit");
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
        if let Some(tor) = &s.tor {
            return OutboundMessage::Ready {
                port: tor.socks_port,
            };
        }
    }

    // Phase-1 strategy: always launch our own Tor. Reusing an existing
    // instance requires the full Control Port handshake landing in Phase 2.
    if let Some(found) = tor_detector::detect_existing().await {
        warn!(
            "detected existing listener on control port {} — ignoring for now \
             (Phase 2 will verify and reuse it)",
            found.control_port
        );
    }

    match launch_owned().await {
        Ok(launched) => {
            let port = launched.socks_port;
            state.lock().await.tor = Some(launched);
            OutboundMessage::Ready { port }
        }
        Err(e) => {
            error!("failed to start Tor: {e:#}");
            OutboundMessage::Error {
                message: format!("{e:#}"),
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
    match s.tor.take() {
        Some(mut tor) => {
            tor_manager::shutdown(&mut tor).await;
            OutboundMessage::Stopped
        }
        None => OutboundMessage::Stopped,
    }
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
