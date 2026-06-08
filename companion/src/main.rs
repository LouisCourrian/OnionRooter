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

mod auth_store;
mod client_auth;
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

/// Native-messaging protocol version. Bump ONLY on protocol changes (not on
/// every companion release). The extension compares it against the minimum it
/// requires; a lower value means "companion too old, please update".
const PROTOCOL_VERSION: u32 = 1;

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
    /// Derived key of the unlocked passphrase vault (None = locked). Held only
    /// in memory for the session; never written to disk.
    pp_key: Option<[u8; 32]>,
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
            protocol: PROTOCOL_VERSION,
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
        InboundMessage::Start => {
            let reply = start(state).await;
            if matches!(reply, OutboundMessage::Ready { .. }) {
                inject_available(state).await;
            }
            reply
        }
        InboundMessage::AuthList { id } => auth_list(state, id).await,
        InboundMessage::AuthAdd {
            id,
            onion,
            label,
            key,
            tier,
        } => auth_add(state, id, onion, label, key, tier).await,
        InboundMessage::AuthRemove { id, onion } => auth_remove(state, id, onion).await,
        InboundMessage::AuthGenerate { id } => auth_generate(id),
        InboundMessage::AuthSetPassphrase { id, passphrase } => {
            auth_set_passphrase(state, id, passphrase).await
        }
        InboundMessage::AuthUnlock { id, passphrase } => auth_unlock(state, id, passphrase).await,
        InboundMessage::AuthLock { id } => auth_lock(state, id).await,
    }
}

// ---------- Client-authorization handlers -----------------------------

fn reply_ok(id: u64, data: Option<serde_json::Value>) -> OutboundMessage {
    OutboundMessage::Reply {
        id,
        ok: true,
        error: None,
        data,
    }
}

fn reply_err(id: u64, error: impl std::fmt::Display) -> OutboundMessage {
    OutboundMessage::Reply {
        id,
        ok: false,
        error: Some(format!("{error:#}")),
        data: None,
    }
}

/// Control port of the active backend, if Tor is running.
async fn active_control_port(state: &Arc<Mutex<State>>) -> Option<u16> {
    state.lock().await.info.as_ref().map(|i| i.control_port)
}

/// Inject every currently-available client key into the running Tor: all OS
/// entries, plus passphrase entries if the vault is unlocked. Best-effort.
async fn inject_available(state: &Arc<Mutex<State>>) {
    let (control_port, pp_key) = {
        let s = state.lock().await;
        match s.info.as_ref() {
            Some(i) => (i.control_port, s.pp_key),
            None => return,
        }
    };

    if let Ok(secrets) = auth_store::os_secrets() {
        for s in secrets {
            if let Err(e) = client_auth::add(control_port, &s.onion, &s.privkey_b64).await {
                warn!("failed to load OS client-auth for {}: {e:#}", s.label);
            }
        }
    }
    if let Some(key) = pp_key {
        if let Ok(secrets) = auth_store::pp_secrets(&key) {
            for s in secrets {
                if let Err(e) = client_auth::add(control_port, &s.onion, &s.privkey_b64).await {
                    warn!("failed to load passphrase client-auth for {}: {e:#}", s.label);
                }
            }
        }
    }
}

async fn auth_list(state: &Arc<Mutex<State>>, id: u64) -> OutboundMessage {
    let (unlocked, running, pp_key) = {
        let s = state.lock().await;
        (s.pp_key.is_some(), s.backend.is_some(), s.pp_key)
    };

    let mut entries = auth_store::list().unwrap_or_default();
    // When unlocked, fill in the real address for passphrase entries so the UI
    // can show/manage them (it only has hashes otherwise).
    if let Some(key) = pp_key {
        if let Ok(secrets) = auth_store::pp_secrets(&key) {
            use std::collections::HashMap;
            let by_hash: HashMap<String, String> = secrets
                .iter()
                .map(|s| (auth_store::onion_hash(&s.onion), s.onion.clone()))
                .collect();
            for e in entries.iter_mut() {
                if e.tier == "passphrase" {
                    if let Some(h) = &e.onion_hash {
                        if let Some(addr) = by_hash.get(h) {
                            e.onion = Some(addr.clone());
                        }
                    }
                }
            }
        }
    }

    let data = serde_json::json!({
        "entries": entries,
        "os_available": auth_store::os_available(),
        "vault_exists": auth_store::vault_exists(),
        "unlocked": unlocked,
        "running": running,
    });
    reply_ok(id, Some(data))
}

async fn auth_add(
    state: &Arc<Mutex<State>>,
    id: u64,
    onion: String,
    label: String,
    key: String,
    tier: String,
) -> OutboundMessage {
    let (parsed_onion, privkey) = match client_auth::normalize_private_key(&key) {
        Ok(v) => v,
        Err(e) => return reply_err(id, e),
    };
    let candidate = if onion.trim().is_empty() {
        parsed_onion
    } else {
        Some(onion)
    };
    let onion = match candidate {
        Some(o) => match client_auth::normalize_onion(&o) {
            Ok(o) => o,
            Err(e) => return reply_err(id, e),
        },
        None => return reply_err(id, "no .onion address provided"),
    };

    let store_result = match tier.as_str() {
        "os" => auth_store::os_add(&onion, &label, &privkey),
        "passphrase" => {
            let key = state.lock().await.pp_key;
            match key {
                Some(k) => auth_store::pp_add(&k, &onion, &label, &privkey),
                None => Err(anyhow::anyhow!("unlock the passphrase vault first")),
            }
        }
        other => Err(anyhow::anyhow!("unknown storage tier: {other}")),
    };
    if let Err(e) = store_result {
        return reply_err(id, e);
    }

    // Load into the running Tor immediately.
    if let Some(cp) = active_control_port(state).await {
        if let Err(e) = client_auth::add(cp, &onion, &privkey).await {
            warn!("stored client-auth but live injection failed: {e:#}");
        }
    }
    reply_ok(id, None)
}

async fn auth_remove(state: &Arc<Mutex<State>>, id: u64, onion: String) -> OutboundMessage {
    let onion = match client_auth::normalize_onion(&onion) {
        Ok(o) => o,
        Err(e) => return reply_err(id, e),
    };
    if let Some(cp) = active_control_port(state).await {
        let _ = client_auth::remove(cp, &onion).await;
    }
    let pp_key = state.lock().await.pp_key;
    match auth_store::remove(&onion, pp_key.as_ref()) {
        Ok(()) => reply_ok(id, None),
        Err(e) => reply_err(id, e),
    }
}

fn auth_generate(id: u64) -> OutboundMessage {
    let (private, public) = client_auth::generate_keypair();
    reply_ok(id, Some(serde_json::json!({ "private": private, "public": public })))
}

async fn auth_set_passphrase(
    state: &Arc<Mutex<State>>,
    id: u64,
    passphrase: String,
) -> OutboundMessage {
    if auth_store::vault_exists() {
        return reply_err(id, "a passphrase vault already exists");
    }
    match auth_store::init_passphrase(&passphrase) {
        Ok(key) => {
            state.lock().await.pp_key = Some(key);
            reply_ok(id, None)
        }
        Err(e) => reply_err(id, e),
    }
}

async fn auth_unlock(state: &Arc<Mutex<State>>, id: u64, passphrase: String) -> OutboundMessage {
    match auth_store::unlock(&passphrase) {
        Ok((key, _secrets)) => {
            state.lock().await.pp_key = Some(key);
            inject_available(state).await;
            reply_ok(id, None)
        }
        Err(e) => reply_err(id, e),
    }
}

async fn auth_lock(state: &Arc<Mutex<State>>, id: u64) -> OutboundMessage {
    // Best-effort: remove passphrase keys from the running Tor so locking
    // takes effect immediately, then drop the in-memory key.
    let (cp, pp_key) = {
        let s = state.lock().await;
        (s.info.as_ref().map(|i| i.control_port), s.pp_key)
    };
    if let (Some(cp), Some(key)) = (cp, pp_key) {
        if let Ok(secrets) = auth_store::pp_secrets(&key) {
            for s in secrets {
                let _ = client_auth::remove(cp, &s.onion).await;
            }
        }
    }
    state.lock().await.pp_key = None;
    reply_ok(id, None)
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
