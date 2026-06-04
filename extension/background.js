// OnionRouter — background script
//
// Responsibilities:
//   1. Maintain a Native Messaging connection to the Rust companion.
//   2. Route requests through Tor according to the active mode:
//        - "onion"     : .onion → Tor, everything else direct
//        - "all"       : everything → Tor
//        - "whitelist" : .onion + user-listed domains → Tor, rest direct
//      .onion ALWAYS goes through Tor regardless of mode (per spec §4.2).
//   3. Drive the toolbar icon: gray (idle) → yellow (starting) →
//      green-tinted-per-mode (ready) → red (error).
//   4. Apply WebRTC settings: forced off in "all" mode, user-controlled
//      in "onion" / "whitelist" mode (per spec §3.2 F17/F18).
//   5. Persist mode + whitelist + WebRTC preference in storage.local.

"use strict";

const COMPANION_HOST = "com.onionrouter.companion";

const MODES = Object.freeze({
  onion: "onion",
  all: "all",
  whitelist: "whitelist",
});

const DEFAULT_SETTINGS = Object.freeze({
  mode: MODES.onion,
  whitelist: [],
  // User-toggled WebRTC kill switch for "onion"/"whitelist" modes.
  // Ignored in "all" mode (where WebRTC is force-disabled regardless).
  webrtcDisabled: false,
});

// Centralised state.
const state = {
  /** "disconnected" | "starting" | "ready" | "error" */
  status: "disconnected",
  /** SOCKS5 port reported by Tor, or null. */
  socksPort: null,
  /** Last human-readable error, or null. */
  errorMessage: null,
  /** Active routing mode. */
  mode: DEFAULT_SETTINGS.mode,
  /** Domains (without protocol/port) routed via Tor in whitelist mode. */
  whitelist: [...DEFAULT_SETTINGS.whitelist],
  /** User pref — only consulted outside "all" mode. */
  webrtcDisabled: DEFAULT_SETTINGS.webrtcDisabled,
};

let companionPort = null;
let readyPromise = null;
let readyResolve = null;
let readyReject = null;

// Pending one-shot request/response waiters for diagnostic + ping round-trips.
// Each entry is { resolve, reject, timer }.
const diagnosticWaiters = new Set();
const pingWaiters = new Set();

// ---------- Settings persistence --------------------------------------

async function loadSettings() {
  const stored = await browser.storage.local.get(["mode", "whitelist", "webrtcDisabled"]);
  state.mode = MODES[stored.mode] || DEFAULT_SETTINGS.mode;
  state.whitelist = Array.isArray(stored.whitelist) ? stored.whitelist.slice() : [];
  state.webrtcDisabled = stored.webrtcDisabled === true;
}

async function saveSetting(key, value) {
  await browser.storage.local.set({ [key]: value });
}

browser.storage.onChanged.addListener((changes, area) => {
  if (area !== "local") return;
  let touched = false;
  if (changes.mode) {
    state.mode = MODES[changes.mode.newValue] || DEFAULT_SETTINGS.mode;
    touched = true;
  }
  if (changes.whitelist) {
    state.whitelist = Array.isArray(changes.whitelist.newValue) ? changes.whitelist.newValue.slice() : [];
    touched = true;
  }
  if (changes.webrtcDisabled) {
    state.webrtcDisabled = changes.webrtcDisabled.newValue === true;
    touched = true;
  }
  if (touched) {
    applyWebRTC();
    refreshIcon();
    broadcastToPopup();
  }
});

// ---------- WebRTC ----------------------------------------------------

/**
 * peerConnectionEnabled = false  → WebRTC OFF (what we want for privacy)
 * peerConnectionEnabled = true   → WebRTC ON (normal Firefox behaviour)
 *
 * We only call clear() when we're not active (i.e. extension disabled)
 * because setting `false` is a controllable setting and Firefox tracks
 * which extension owns it.
 */
async function applyWebRTC() {
  const shouldDisable = state.mode === MODES.all || state.webrtcDisabled === true;
  try {
    if (shouldDisable) {
      await browser.privacy.network.peerConnectionEnabled.set({ value: false });
    } else {
      await browser.privacy.network.peerConnectionEnabled.clear({});
    }
  } catch (err) {
    console.warn("[OnionRouter] could not adjust WebRTC setting:", err && err.message);
  }
}

// ---------- Companion connection --------------------------------------

/**
 * Open the Native Messaging port (spawning the companion) WITHOUT starting
 * Tor. Used by the diagnostic page / ping test, which must be able to talk
 * to the companion without forcing a Tor launch. Returns true on success.
 */
function ensureCompanionPort() {
  if (companionPort) return true;
  try {
    companionPort = browser.runtime.connectNative(COMPANION_HOST);
  } catch (err) {
    console.error("[OnionRouter] connectNative threw:", err);
    setStatus("error", { errorMessage: String((err && err.message)) || "connectNative failed" });
    return false;
  }
  companionPort.onMessage.addListener(onCompanionMessage);
  companionPort.onDisconnect.addListener(onCompanionDisconnect);
  return true;
}

function connectCompanion() {
  if (companionPort) return;
  console.info("[OnionRouter] connecting to companion", COMPANION_HOST);

  setStatus("starting", { errorMessage: null });

  if (!ensureCompanionPort()) return;
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

function settleReady(value, error) {
  if (error && readyReject) readyReject(error);
  else if (!error && readyResolve) readyResolve(value);
  readyPromise = null;
  readyResolve = null;
  readyReject = null;
}

// ---------- Diagnostic / ping round-trips -----------------------------

function settleWaiters(set, value) {
  for (const w of set) {
    clearTimeout(w.timer);
    w.resolve(value);
  }
  set.clear();
}

function rejectWaiters(set, error) {
  for (const w of set) {
    clearTimeout(w.timer);
    w.reject(error);
  }
  set.clear();
}

/** Round-trips one request to the companion, resolving when `set` settles. */
function companionRequest(set, payload, timeoutMs = 4000) {
  if (!ensureCompanionPort()) {
    return Promise.reject(new Error("companion unavailable"));
  }
  return new Promise((resolve, reject) => {
    const waiter = { resolve, reject, timer: null };
    waiter.timer = setTimeout(() => {
      set.delete(waiter);
      reject(new Error("companion did not respond in time"));
    }, timeoutMs);
    set.add(waiter);
    if (!send(payload)) {
      clearTimeout(waiter.timer);
      set.delete(waiter);
      reject(new Error("could not send request to companion"));
    }
  });
}

function requestDiagnostic() {
  return companionRequest(diagnosticWaiters, { action: "diagnostic" });
}

function pingCompanion() {
  return companionRequest(pingWaiters, { action: "ping" });
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
        settleReady(msg.port, null);
      }
      break;
    case "stopped":
      setStatus("disconnected", { socksPort: null });
      // A pending waiter shouldn't hang if the user clicks Stop mid-bootstrap.
      settleReady(null, new Error("Tor was stopped"));
      break;
    case "error": {
      const m = msg.message || "unknown error";
      setStatus("error", { errorMessage: m });
      settleReady(null, new Error(m));
      break;
    }
    case "pong":
      settleWaiters(pingWaiters, true);
      break;
    case "diagnostic":
      settleWaiters(diagnosticWaiters, msg);
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
    settleReady(null, new Error(error.message));
  } else {
    console.info("[OnionRouter] companion disconnected");
    setStatus("disconnected", { socksPort: null });
    settleReady(null, new Error("companion disconnected"));
  }
  const disconnectError = new Error((error && error.message) || "companion disconnected");
  rejectWaiters(diagnosticWaiters, disconnectError);
  rejectWaiters(pingWaiters, disconnectError);
  companionPort = null;
}

/**
 * Returns a promise that resolves with the SOCKS port once Tor is ready,
 * or rejects if Tor enters an error state / is stopped / the companion
 * disconnects. Callers wanting to block a request until Tor is up should
 * await this directly.
 */
function waitForReady() {
  if (state.status === "ready" && state.socksPort) {
    return Promise.resolve(state.socksPort);
  }
  if (state.status === "error") {
    return Promise.reject(new Error(state.errorMessage || "Tor unavailable"));
  }
  if (!readyPromise) {
    readyPromise = new Promise((resolve, reject) => {
      readyResolve = resolve;
      readyReject = reject;
    });
  }
  if (!companionPort) connectCompanion();
  else if (state.status !== "starting" && state.status !== "ready") send({ action: "start" });
  return readyPromise;
}

// ---------- Routing ---------------------------------------------------

function isOnion(hostname) {
  if (!hostname) return false;
  const h = hostname.endsWith(".") ? hostname.slice(0, -1) : hostname;
  return h.toLowerCase().endsWith(".onion");
}

/**
 * Whitelist matches the exact domain OR any subdomain.
 *   addDomain "example.com" matches:
 *     - example.com
 *     - www.example.com
 *     - any.deeply.nested.example.com
 *   ...but NOT "evilexample.com" (suffix-only is unsafe).
 */
function matchesWhitelist(hostname, list) {
  if (!hostname || !list || list.length === 0) return false;
  const h = (hostname.endsWith(".") ? hostname.slice(0, -1) : hostname).toLowerCase();
  return list.some((entry) => {
    if (!entry) return false;
    const d = String(entry).toLowerCase().replace(/^\.+/, "");
    return h === d || h.endsWith("." + d);
  });
}

function shouldUseTor(url) {
  let host;
  try {
    host = new URL(url).hostname;
  } catch {
    return false;
  }
  // .onion is non-negotiable in every mode.
  if (isOnion(host)) return true;

  switch (state.mode) {
    case MODES.all:
      return true;
    case MODES.whitelist:
      return matchesWhitelist(host, state.whitelist);
    case MODES.onion:
    default:
      return false;
  }
}

function torProxyInfo(port) {
  return {
    type: "socks",
    host: "127.0.0.1",
    port,
    proxyDNS: true, // forces DNS through Tor — prevents DNS leaks
  };
}

/** Returned when Tor failed irrecoverably and we must NOT leak the host. */
const UNREACHABLE_PROXY = Object.freeze({
  type: "socks",
  host: "127.0.0.1",
  port: 1,
  proxyDNS: true,
  failoverTimeout: 1,
});

/**
 * Firefox accepts a Promise from proxy.onRequest. Resolving it pauses the
 * request until Tor is up, which gives the user a "slow load" feel instead
 * of an instant fail-to-load on cold start. If Tor never comes up, the
 * promise rejects and we substitute an unreachable proxy so the .onion
 * host name doesn't leak via direct DNS.
 */
function handleProxyRequest(requestInfo) {
  if (!shouldUseTor(requestInfo.url)) return { type: "direct" };

  if (state.status === "ready" && state.socksPort) {
    return torProxyInfo(state.socksPort);
  }

  return waitForReady()
    .then((port) => torProxyInfo(port))
    .catch((err) => {
      console.warn("[OnionRouter] waitForReady failed for", requestInfo.url, err);
      return UNREACHABLE_PROXY;
    });
}

browser.proxy.onRequest.addListener(handleProxyRequest, { urls: ["<all_urls>"] });

// ---------- Toolbar icon ----------------------------------------------

const READY_ICON_BY_MODE = {
  [MODES.onion]: "icons/icon-active-onion.svg",
  [MODES.all]: "icons/icon-active-all.svg",
  [MODES.whitelist]: "icons/icon-active-whitelist.svg",
};

function iconPath() {
  if (state.status === "ready") {
    return READY_ICON_BY_MODE[state.mode] || READY_ICON_BY_MODE[MODES.onion];
  }
  if (state.status === "starting") return "icons/icon-starting.svg";
  if (state.status === "error") return "icons/icon-error.svg";
  return "icons/icon-inactive.svg";
}

function statusLabel() {
  if (state.status === "ready") {
    switch (state.mode) {
      case MODES.all:
        return `OnionRouter — Tor active (all traffic, port ${state.socksPort})`;
      case MODES.whitelist:
        return `OnionRouter — Tor active (whitelist, port ${state.socksPort})`;
      default:
        return `OnionRouter — Tor active (onion-only, port ${state.socksPort})`;
    }
  }
  if (state.status === "starting") return "OnionRouter — starting Tor…";
  if (state.status === "error") return `OnionRouter — error: ${state.errorMessage || "unknown"}`;
  return "OnionRouter — inactive";
}

function refreshIcon() {
  browser.action.setIcon({ path: iconPath() });
  browser.action.setTitle({ title: statusLabel() });
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
  return {
    status: state.status,
    socksPort: state.socksPort,
    errorMessage: state.errorMessage,
    mode: state.mode,
    whitelist: state.whitelist.slice(),
    webrtcDisabled: state.webrtcDisabled,
  };
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

// ---------- Whitelist mutation helpers --------------------------------

function normalizeDomain(input) {
  if (!input) return null;
  let s = String(input).trim().toLowerCase();
  if (!s) return null;
  // Strip scheme + path if user pasted a full URL.
  try {
    if (s.includes("://")) s = new URL(s).hostname;
  } catch {
    /* keep raw input */
  }
  // Strip port and userinfo just in case.
  s = s.split("/")[0].split(":")[0].replace(/^\.+/, "").replace(/\.+$/, "");
  // Basic sanity: must contain a dot and only contain hostname-allowed chars.
  if (!s.includes(".")) return null;
  if (!/^[a-z0-9.-]+$/.test(s)) return null;
  return s;
}

async function addToWhitelist(domain) {
  const norm = normalizeDomain(domain);
  if (!norm) return { ok: false, reason: "invalid domain" };
  const next = state.whitelist.slice();
  if (!next.includes(norm)) next.push(norm);
  next.sort();
  await saveSetting("whitelist", next);
  return { ok: true, whitelist: next };
}

async function removeFromWhitelist(domain) {
  const next = state.whitelist.filter((d) => d !== domain);
  await saveSetting("whitelist", next);
  return { ok: true, whitelist: next };
}

async function addCurrentTabToWhitelist() {
  const tabs = await browser.tabs.query({ active: true, currentWindow: true });
  const url = tabs[0] && tabs[0].url;
  if (!url) return { ok: false, reason: "no active tab" };
  let host;
  try {
    host = new URL(url).hostname;
  } catch {
    return { ok: false, reason: "tab has no hostname" };
  }
  return addToWhitelist(host);
}

// ---------- Runtime message router ------------------------------------

browser.runtime.onMessage.addListener((msg) => {
  if (!msg || typeof msg.type !== "string") return undefined;
  switch (msg.type) {
    case "get-state":
      return Promise.resolve(getState());
    case "get-diagnostic":
      return requestDiagnostic().then(
        (diagnostic) => ({ ok: true, diagnostic, state: getState() }),
        (err) => ({ ok: false, reason: String((err && err.message) || err), state: getState() })
      );
    case "ping-companion":
      return pingCompanion().then(
        () => ({ ok: true }),
        (err) => ({ ok: false, reason: String((err && err.message) || err) })
      );
    case "start-tor":
      connectCompanion();
      return Promise.resolve(getState());
    case "stop-tor":
      send({ action: "stop" });
      return Promise.resolve(getState());
    case "set-mode": {
      const m = MODES[msg.mode];
      if (!m) return Promise.resolve({ ok: false, reason: "invalid mode" });
      return saveSetting("mode", m).then(() => ({ ok: true, mode: m }));
    }
    case "set-webrtc":
      return saveSetting("webrtcDisabled", msg.value === true).then(() => ({ ok: true }));
    case "whitelist-add":
      return addToWhitelist(msg.domain);
    case "whitelist-add-current":
      return addCurrentTabToWhitelist();
    case "whitelist-remove":
      return removeFromWhitelist(msg.domain);
    default:
      return undefined;
  }
});

// ---------- Boot -------------------------------------------------------

(async () => {
  await loadSettings();
  await applyWebRTC();
  refreshIcon();
})();
