// OnionRouter — diagnostics page.
//
// Pulls the extension's own state plus a fresh diagnostic snapshot from the
// companion, and lets the user run a live ping test and copy a plain-text
// report for bug reports.

"use strict";

const STATUS_LABELS = {
  disconnected: "Inactive",
  starting: "Starting…",
  ready: "Active",
  error: "Error",
};

const MODE_LABELS = {
  onion: "Onion only (.onion → Tor)",
  all: "All traffic via Tor",
  whitelist: "Whitelist",
};

const SOURCE_LABELS = {
  owned: "Launched by companion",
  tray: "Tray daemon",
  external: "Reused external Tor",
};

const els = {
  statusPill: document.getElementById("status-pill"),

  companionConn: document.getElementById("companion-conn"),
  companionVersion: document.getElementById("companion-version"),
  protocol: document.getElementById("protocol"),
  platform: document.getElementById("platform"),

  torRunning: document.getElementById("tor-running"),
  torSource: document.getElementById("tor-source"),
  torSocks: document.getElementById("tor-socks"),
  torControl: document.getElementById("tor-control"),
  torVersion: document.getElementById("tor-version"),
  bundleVersion: document.getElementById("bundle-version"),
  dataDir: document.getElementById("data-dir"),

  extVersion: document.getElementById("ext-version"),
  extStatus: document.getElementById("ext-status"),
  extMode: document.getElementById("ext-mode"),
  extWebrtc: document.getElementById("ext-webrtc"),
  extWhitelist: document.getElementById("ext-whitelist"),
  extErrorRow: document.getElementById("ext-error-row"),
  extError: document.getElementById("ext-error"),

  refreshBtn: document.getElementById("refresh-btn"),
  pingBtn: document.getElementById("ping-btn"),
  copyBtn: document.getElementById("copy-btn"),
  hint: document.getElementById("action-hint"),
};

// Latest values, kept for the "Copy report" action.
const snapshot = { state: null, diagnostic: null, connection: "unknown" };

function setText(el, value, dataState) {
  const empty = value === null || value === undefined || value === "";
  el.textContent = empty ? "—" : String(value);
  if (dataState) el.dataset.state = dataState;
  else el.removeAttribute("data-state");
}

function webrtcLabel(s) {
  if (s.mode === "all") return "Off (forced in All-via-Tor mode)";
  return s.webrtcDisabled ? "Off (user)" : "Firefox default";
}

function renderState(s) {
  snapshot.state = s;

  els.statusPill.dataset.status = s.status;
  els.statusPill.textContent = STATUS_LABELS[s.status] || s.status;

  const manifest = browser.runtime.getManifest();
  setText(els.extVersion, manifest.version);
  setText(els.extStatus, STATUS_LABELS[s.status] || s.status);
  setText(els.extMode, MODE_LABELS[s.mode] || s.mode);
  setText(els.extWebrtc, webrtcLabel(s));
  setText(els.extWhitelist, (s.whitelist || []).length);

  if (s.errorMessage) {
    els.extErrorRow.hidden = false;
    setText(els.extError, s.errorMessage, "bad");
  } else {
    els.extErrorRow.hidden = true;
  }
}

function renderDiagnostic(result) {
  if (result && result.ok && result.diagnostic) {
    const d = result.diagnostic;
    snapshot.diagnostic = d;
    snapshot.connection = "connected";

    setText(els.companionConn, "Connected", "ok");
    setText(els.companionVersion, d.companion_version);
    const outdated = snapshot.state && snapshot.state.companionOutdated;
    setText(
      els.protocol,
      d.protocol != null ? "v" + d.protocol + (outdated ? " — outdated, update the companion" : "") : null,
      outdated ? "bad" : null
    );
    setText(els.platform, d.platform);

    setText(els.torRunning, d.running ? "Yes" : "No", d.running ? "ok" : null);
    setText(els.torSource, d.source ? (SOURCE_LABELS[d.source] || d.source) : "—");
    setText(els.torSocks, d.socks_port);
    setText(els.torControl, d.control_port);
    setText(els.torVersion, d.tor_version);
    setText(els.bundleVersion, d.bundle_version);
    setText(els.dataDir, d.data_dir);
  } else {
    snapshot.diagnostic = null;
    snapshot.connection = "unreachable";
    const reason = (result && result.reason) || "no response";
    setText(els.companionConn, `Not responding (${reason})`, "bad");
    // Leave companion/Tor fields blank — we have no data.
    for (const el of [
      els.companionVersion, els.protocol, els.platform, els.torRunning, els.torSource,
      els.torSocks, els.torControl, els.torVersion, els.bundleVersion, els.dataDir,
    ]) {
      setText(el, null);
    }
  }
}

async function refresh() {
  els.refreshBtn.disabled = true;
  els.hint.textContent = "Refreshing…";
  try {
    const state = await browser.runtime.sendMessage({ type: "get-state" });
    if (state) renderState(state);
    const diag = await browser.runtime.sendMessage({ type: "get-diagnostic" });
    renderDiagnostic(diag);
    if (diag && diag.state) renderState(diag.state);
    els.hint.textContent = "Updated.";
  } catch (err) {
    els.hint.textContent = "Refresh failed: " + ((err && err.message) || err);
  } finally {
    els.refreshBtn.disabled = false;
  }
}

async function runPing() {
  els.pingBtn.disabled = true;
  els.hint.textContent = "Pinging companion…";
  const t0 = Date.now();
  try {
    const res = await browser.runtime.sendMessage({ type: "ping-companion" });
    if (res && res.ok) {
      const ms = Date.now() - t0;
      els.hint.textContent = `Companion replied (pong) in ${ms} ms.`;
      setText(els.companionConn, "Connected", "ok");
      snapshot.connection = "connected";
    } else {
      const reason = (res && res.reason) || "no response";
      els.hint.textContent = "Ping failed: " + reason;
      setText(els.companionConn, `Not responding (${reason})`, "bad");
      snapshot.connection = "unreachable";
    }
  } catch (err) {
    els.hint.textContent = "Ping failed: " + ((err && err.message) || err);
  } finally {
    els.pingBtn.disabled = false;
  }
}

function buildReport() {
  const s = snapshot.state || {};
  const d = snapshot.diagnostic || {};
  const manifest = browser.runtime.getManifest();
  const line = (k, v) => `${k}: ${v === null || v === undefined || v === "" ? "n/a" : v}`;
  return [
    "OnionRouter diagnostic report",
    "=============================",
    line("Extension version", manifest.version),
    line("Companion connection", snapshot.connection),
    line("Companion version", d.companion_version),
    line("Platform", d.platform),
    "",
    line("Tor running", d.running === undefined ? "n/a" : (d.running ? "yes" : "no")),
    line("Tor source", d.source ? (SOURCE_LABELS[d.source] || d.source) : "n/a"),
    line("SOCKS port", d.socks_port),
    line("Control port", d.control_port),
    line("Tor version", d.tor_version),
    line("Bundle version", d.bundle_version),
    line("Data directory", d.data_dir),
    "",
    line("Status", STATUS_LABELS[s.status] || s.status),
    line("Routing mode", MODE_LABELS[s.mode] || s.mode),
    line("WebRTC", s.mode ? webrtcLabel(s) : "n/a"),
    line("Whitelisted domains", (s.whitelist || []).length),
    line("Last error", s.errorMessage || "none"),
  ].join("\n");
}

async function copyReport() {
  const text = buildReport();
  try {
    await navigator.clipboard.writeText(text);
    els.hint.textContent = "Report copied to clipboard.";
  } catch {
    // Fallback for environments where the async clipboard API is blocked.
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand("copy");
      els.hint.textContent = "Report copied to clipboard.";
    } catch {
      els.hint.textContent = "Could not copy automatically — select and copy manually.";
    }
    ta.remove();
  }
}

els.refreshBtn.addEventListener("click", refresh);
els.pingBtn.addEventListener("click", runPing);
els.copyBtn.addEventListener("click", copyReport);

// Live status updates while the page is open.
const port = browser.runtime.connect({ name: "popup" });
port.onMessage.addListener((msg) => {
  if (msg && msg.type === "state") renderState(msg.state);
});

refresh();
