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

// Minimum native-messaging protocol this extension needs from the companion.
// The companion and the extension are versioned independently (the extension
// auto-updates via AMO, the companion via its installer), so a newer extension
// can meet an older companion. If the companion reports a lower protocol, we
// surface an "update the companion" prompt.
const REQUIRED_PROTOCOL = 1;

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
  /** Protocol version reported by the companion (null until known). */
  companionProtocol: null,
  /** Companion version string (null until known). */
  companionVersion: null,
  /** Latest companion-update info from the companion (checked via Tor). */
  companionUpdate: null,
};

let companionPort = null;
let readyPromise = null;
let readyResolve = null;
let readyReject = null;

// Pending one-shot request/response waiters for diagnostic + ping round-trips.
// Each entry is { resolve, reject, timer }.
const diagnosticWaiters = new Set();
const pingWaiters = new Set();

// id-correlated request/response for client-auth ("auth-*") actions.
let nextRequestId = 1;
const pendingRequests = new Map(); // id -> { resolve, reject, timer }

// Cached client-auth index (non-secret metadata). Persisted to storage.local
// so the webRequest interceptor works at startup without spawning the
// companion. `unlocked` is in-memory only (never persisted): a fresh companion
// session always starts locked.
let authIndex = { entries: [], osAvailable: false, vaultExists: false, unlocked: false };
// Resolves once the cached index has been loaded from storage. The webRequest
// interceptor awaits it so a freshly-woken event page doesn't miss a redirect.
let authIndexReady = null;

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

/** Learn the companion's protocol + version once per session (for skew UX). */
async function cacheCompanionInfo() {
  if (state.companionProtocol !== null) return;
  try {
    const d = await requestDiagnostic();
    if (d && typeof d.protocol === "number") {
      state.companionProtocol = d.protocol;
      state.companionVersion = d.companion_version || null;
      broadcastToPopup();
    }
  } catch {
    /* best-effort */
  }
}

/**
 * Ask the companion to check for a newer companion release. The companion
 * performs the GitHub request **through Tor**, so the user's real IP is never
 * exposed. Requires Tor to be running.
 */
async function checkForUpdate(force = false) {
  if (state.status !== "ready") return; // needs Tor up to route via Tor
  if (!force && state.companionUpdate) return; // already checked this session
  try {
    const r = await companionRequestId({ action: "update-check" }, 40000);
    if (r && r.ok && r.data) {
      state.companionUpdate = r.data;
      broadcastToPopup();
    }
  } catch {
    /* best-effort */
  }
}

// Daily background check (only runs when Tor is up).
browser.alarms.create("companion-update-check", { periodInMinutes: 1440 });
browser.alarms.onAlarm.addListener((a) => {
  if (a.name === "companion-update-check") checkForUpdate(true);
});

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
        cacheCompanionInfo();
        checkForUpdate();
        // Keep the cached credential index fresh (non-blocking), so the unlock
        // interceptor sees up-to-date entries on later navigations.
        refreshAuthIndex().catch(() => {});
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
    case "reply": {
      const w = pendingRequests.get(msg.id);
      if (w) {
        clearTimeout(w.timer);
        pendingRequests.delete(msg.id);
        w.resolve(msg);
      }
      break;
    }
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
  for (const [, w] of pendingRequests) {
    clearTimeout(w.timer);
    w.reject(disconnectError);
  }
  pendingRequests.clear();
  // A new companion session starts locked; re-learn protocol next time.
  authIndex.unlocked = false;
  state.companionProtocol = null;
  state.companionVersion = null;
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
  [MODES.onion]: "icons/icon-active-onion.png",
  [MODES.all]: "icons/icon-active-all.png",
  [MODES.whitelist]: "icons/icon-active-whitelist.png",
};

function iconPath() {
  if (state.status === "ready") {
    return READY_ICON_BY_MODE[state.mode] || READY_ICON_BY_MODE[MODES.onion];
  }
  if (state.status === "starting") return "icons/icon-starting.png";
  if (state.status === "error") return "icons/icon-error.png";
  return "icons/icon-inactive.png";
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
    companionProtocol: state.companionProtocol,
    companionVersion: state.companionVersion,
    requiredProtocol: REQUIRED_PROTOCOL,
    companionOutdated:
      state.companionProtocol !== null && state.companionProtocol < REQUIRED_PROTOCOL,
    companionUpdateAvailable: !!(state.companionUpdate && state.companionUpdate.update_available),
    latestCompanionVersion: state.companionUpdate ? state.companionUpdate.latest : null,
    companionUpdateUrl: state.companionUpdate ? state.companionUpdate.url : null,
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
    case "auth":
      // { type:"auth", payload:{ action:"auth-...", ... } }
      return handleAuth(msg.payload || {});
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

// ---------- Client authorization (v3 onion auth) ----------------------

/** Send an id-correlated request to the companion and await its `reply`. */
function companionRequestId(payload, timeoutMs = 20000) {
  if (!ensureCompanionPort()) {
    return Promise.reject(new Error("companion unavailable"));
  }
  return new Promise((resolve, reject) => {
    const id = nextRequestId++;
    const timer = setTimeout(() => {
      pendingRequests.delete(id);
      reject(new Error("companion did not respond in time"));
    }, timeoutMs);
    pendingRequests.set(id, { resolve, reject, timer });
    if (!send({ ...payload, id })) {
      clearTimeout(timer);
      pendingRequests.delete(id);
      reject(new Error("could not send request to companion"));
    }
  });
}

/** Pull the latest index from the companion and cache it (memory + storage). */
async function refreshAuthIndex() {
  const r = await companionRequestId({ action: "auth-list" });
  if (r && r.ok && r.data) {
    authIndex = {
      entries: Array.isArray(r.data.entries) ? r.data.entries : [],
      osAvailable: !!r.data.os_available,
      vaultExists: !!r.data.vault_exists,
      unlocked: !!r.data.unlocked,
    };
    await browser.storage.local.set({
      authIndex: {
        entries: authIndex.entries,
        osAvailable: authIndex.osAvailable,
        vaultExists: authIndex.vaultExists,
      },
    });
  }
  return r;
}

/** Restore the cached index at startup (no companion spawn). */
async function loadAuthIndex() {
  try {
    const stored = (await browser.storage.local.get("authIndex")).authIndex;
    if (stored) {
      authIndex = {
        entries: Array.isArray(stored.entries) ? stored.entries : [],
        osAvailable: !!stored.osAvailable,
        vaultExists: !!stored.vaultExists,
        unlocked: false,
      };
    }
  } catch {
    /* ignore */
  }
}

async function sha256Hex(input) {
  const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(input));
  return Array.from(new Uint8Array(buf))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * Intercept top-level navigations to a passphrase-protected .onion that is
 * still locked, and redirect to the unlock page. After unlocking, that page
 * sends the browser back to the original URL (now reachable).
 */
async function onBeforeOnionRequest(details) {
  let host;
  try {
    host = new URL(details.url).hostname.replace(/\.$/, "").toLowerCase();
  } catch {
    return {};
  }
  if (!host.endsWith(".onion")) return {};

  // Make sure the cached index is loaded (the event page may have just woken
  // up). This is cheap (storage read) -- we must NOT do a companion round-trip
  // here: this is a blocking listener on every .onion navigation.
  try {
    await authIndexReady;
  } catch {
    /* best-effort */
  }

  const onion = host.slice(0, -".onion".length);
  const protectedEntries = authIndex.entries.filter(
    (e) => e.tier === "passphrase" && e.onion_hash
  );
  const hash = await sha256Hex(onion);
  const entry = protectedEntries.find((e) => e.onion_hash === hash);
  if (authIndex.unlocked || !entry) return {};

  const url =
    browser.runtime.getURL("unlock.html") +
    "?onion=" + encodeURIComponent(onion) +
    "&label=" + encodeURIComponent(entry.label || "") +
    "&return=" + encodeURIComponent(details.url);
  return { redirectUrl: url };
}

// Kick off the cached-index load at top level so it runs on every event-page
// wake (not only in the boot IIFE), and the interceptor can await it.
authIndexReady = loadAuthIndex();

browser.webRequest.onBeforeRequest.addListener(
  onBeforeOnionRequest,
  { urls: ["*://*.onion/*"], types: ["main_frame"] },
  ["blocking"]
);

/** Forward an auth command to the companion, refreshing the cache on changes. */
async function handleAuth(payload) {
  try {
    const reply = await companionRequestId(payload);
    const mutating = [
      "auth-add",
      "auth-remove",
      "auth-unlock",
      "auth-lock",
      "auth-set-passphrase",
    ];
    if (reply && reply.ok && mutating.includes(payload.action)) {
      if (payload.action === "auth-unlock" || payload.action === "auth-set-passphrase") {
        authIndex.unlocked = true;
      } else if (payload.action === "auth-lock") {
        authIndex.unlocked = false;
      }
      await refreshAuthIndex();
    }
    return reply;
  } catch (e) {
    return { ok: false, error: String((e && e.message) || e) };
  }
}

// ---------- First-run welcome (F10) -----------------------------------

// Shown once, right after the extension is installed. Explains that Tor is
// fetched transparently on first use so the initial "slow load" isn't
// mistaken for a failure. Only fires on a fresh install, not on updates.
browser.runtime.onInstalled.addListener((details) => {
  if (details.reason !== "install") return;
  try {
    browser.notifications.create("onionrouter-welcome", {
      type: "basic",
      iconUrl: browser.runtime.getURL("icons/icon-active-onion.png"),
      title: "OnionRouter is ready",
      message:
        "Open any .onion address and Tor starts automatically — it's " +
        "downloaded and verified in the background on first use, so the very " +
        "first load can take a moment. Click the onion toolbar icon to pick a " +
        "routing mode.",
    });
  } catch (err) {
    console.warn("[OnionRouter] welcome notification failed:", err && err.message);
  }
});

// ---------- Boot -------------------------------------------------------

(async () => {
  await loadSettings();
  await authIndexReady;
  await applyWebRTC();
  refreshIcon();
})();
