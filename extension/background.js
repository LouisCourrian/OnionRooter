// OnionRouter — background script
//
// Responsibilities:
//   1. Maintain a Native Messaging connection to the Rust companion.
//   2. Detect .onion URLs and route them through the SOCKS5 port exposed
//      by Tor (port reported by the companion).
//   3. Drive the toolbar icon state (gray → yellow → green).
//
// Phase 1 scope: "Onion only" mode hardcoded — .onion goes through Tor,
// everything else goes direct. Modes "All via Tor" and "Whitelist" arrive
// in Phase 3.

"use strict";

const COMPANION_HOST = "com.onionrouter.companion";

// Centralised state. Everything that reads this object must do so through
// `getState()` so we have one place to add observers later.
const state = {
  /** "disconnected" | "starting" | "ready" | "error" */
  status: "disconnected",
  /** SOCKS5 port reported by Tor, or null. */
  socksPort: null,
  /** Last human-readable error, or null. */
  errorMessage: null,
};

let companionPort = null;
/** Resolves when the next "ready" message arrives. Reset on disconnect. */
let readyPromise = null;
let readyResolve = null;

// ---------- Companion connection ---------------------------------------

function connectCompanion() {
  if (companionPort) return;
  console.info("[OnionRouter] connecting to companion", COMPANION_HOST);

  setStatus("starting", { errorMessage: null });

  try {
    companionPort = browser.runtime.connectNative(COMPANION_HOST);
  } catch (err) {
    console.error("[OnionRouter] connectNative threw:", err);
    setStatus("error", { errorMessage: String(err && err.message) || "connectNative failed" });
    return;
  }

  companionPort.onMessage.addListener(onCompanionMessage);
  companionPort.onDisconnect.addListener(onCompanionDisconnect);

  // Kick the companion to launch Tor immediately.
  send({ action: "start" });
}

function send(payload) {
  if (!companionPort) return false;
  try {
    companionPort.postMessage(payload);
    return true;
  } catch (err) {
    console.error("[OnionRouter] postMessage failed:", err);
    return false;
  }
}

function onCompanionMessage(msg) {
  console.debug("[OnionRouter] ←", msg);
  switch (msg && msg.status) {
    case "starting":
      setStatus("starting");
      break;
    case "ready":
      if (typeof msg.port === "number") {
        setStatus("ready", { socksPort: msg.port, errorMessage: null });
        if (readyResolve) {
          readyResolve(msg.port);
          readyResolve = null;
          readyPromise = null;
        }
      }
      break;
    case "stopped":
      setStatus("disconnected", { socksPort: null });
      break;
    case "error":
      setStatus("error", { errorMessage: msg.message || "unknown error" });
      break;
    case "pong":
      // health check — no state change
      break;
    default:
      console.warn("[OnionRouter] unknown companion message:", msg);
  }
}

function onCompanionDisconnect(port) {
  const error = port && port.error;
  if (error) {
    console.warn("[OnionRouter] companion disconnected with error:", error.message);
    setStatus("error", { errorMessage: error.message });
  } else {
    console.info("[OnionRouter] companion disconnected");
    setStatus("disconnected", { socksPort: null });
  }
  companionPort = null;
  readyResolve = null;
  readyPromise = null;
}

/** Returns a promise that resolves with the SOCKS port once Tor is ready. */
function waitForReady() {
  if (state.status === "ready" && state.socksPort) {
    return Promise.resolve(state.socksPort);
  }
  if (!readyPromise) {
    readyPromise = new Promise((resolve) => {
      readyResolve = resolve;
    });
  }
  // Make sure we're at least connected and asking for a start.
  if (!companionPort) connectCompanion();
  else if (state.status !== "starting" && state.status !== "ready") send({ action: "start" });
  return readyPromise;
}

// ---------- Proxy routing ---------------------------------------------

function isOnion(hostname) {
  if (!hostname) return false;
  // Strip a trailing dot from FQDN form like "example.onion."
  const h = hostname.endsWith(".") ? hostname.slice(0, -1) : hostname;
  return h.toLowerCase().endsWith(".onion");
}

/**
 * proxy.onRequest handler.
 * Return a ProxyInfo (or array) — Firefox honours the first reachable one.
 * Returning `{ type: "direct" }` means "no proxy".
 */
function handleProxyRequest(requestInfo) {
  let host;
  try {
    host = new URL(requestInfo.url).hostname;
  } catch {
    return { type: "direct" };
  }

  if (!isOnion(host)) return { type: "direct" };

  // .onion → Tor. If Tor isn't ready yet, returning "direct" would leak
  // a DNS lookup for a hostname that doesn't resolve. Returning an
  // unreachable proxy makes Firefox fail the request cleanly instead.
  const port = state.socksPort;
  if (state.status !== "ready" || !port) {
    // Trigger a connection attempt so the next request succeeds.
    void waitForReady();
    return {
      type: "socks",
      host: "127.0.0.1",
      port: 1, // intentionally invalid — request fails fast, no DNS leak
      proxyDNS: true,
      failoverTimeout: 1,
    };
  }

  return {
    type: "socks",
    host: "127.0.0.1",
    port,
    proxyDNS: true, // forces DNS resolution through Tor — protects against DNS leaks
  };
}

browser.proxy.onRequest.addListener(handleProxyRequest, { urls: ["<all_urls>"] });

// ---------- Toolbar icon ----------------------------------------------

const ICONS = {
  disconnected: "icons/icon-inactive.svg",
  starting: "icons/icon-starting.svg",
  ready: "icons/icon-active-onion.svg",
  error: "icons/icon-error.svg",
};

function refreshIcon() {
  browser.action.setIcon({ path: ICONS[state.status] || ICONS.disconnected });

  const title =
    state.status === "ready"
      ? `OnionRouter — Tor active (SOCKS port ${state.socksPort})`
      : state.status === "starting"
      ? "OnionRouter — starting Tor…"
      : state.status === "error"
      ? `OnionRouter — error: ${state.errorMessage || "unknown"}`
      : "OnionRouter — inactive";
  browser.action.setTitle({ title });
}

// ---------- State plumbing --------------------------------------------

function setStatus(status, patch = {}) {
  state.status = status;
  if ("socksPort" in patch) state.socksPort = patch.socksPort;
  if ("errorMessage" in patch) state.errorMessage = patch.errorMessage;
  refreshIcon();
  broadcastToPopup();
}

function getState() {
  return { ...state };
}

// ---------- Popup messaging -------------------------------------------

const popupSubscribers = new Set();

browser.runtime.onConnect.addListener((port) => {
  if (port.name !== "popup") return;
  popupSubscribers.add(port);
  port.postMessage({ type: "state", state: getState() });
  port.onDisconnect.addListener(() => popupSubscribers.delete(port));
});

function broadcastToPopup() {
  const payload = { type: "state", state: getState() };
  for (const port of popupSubscribers) {
    try {
      port.postMessage(payload);
    } catch {
      popupSubscribers.delete(port);
    }
  }
}

browser.runtime.onMessage.addListener((msg) => {
  switch (msg && msg.type) {
    case "get-state":
      return Promise.resolve(getState());
    case "start-tor":
      connectCompanion();
      return Promise.resolve(getState());
    case "stop-tor":
      send({ action: "stop" });
      return Promise.resolve(getState());
    default:
      return undefined;
  }
});

// ---------- Boot -------------------------------------------------------

refreshIcon();
// Lazy connect: only spin up the companion when an .onion is actually
// requested. Uncomment below to connect eagerly at browser start.
// connectCompanion();
