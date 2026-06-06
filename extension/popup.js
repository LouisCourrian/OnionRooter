// OnionRouter popup — subscribes to background state and lets the user
// switch routing modes, manage the whitelist, and toggle WebRTC.

"use strict";

const STATUS_LABELS = {
  disconnected: "Inactive",
  starting: "Starting…",
  ready: "Active",
  error: "Error",
};

const els = {
  statusPill: document.getElementById("status-pill"),
  socksPort: document.getElementById("socks-port"),
  errorRow: document.getElementById("error-row"),
  errorMessage: document.getElementById("error-message"),

  modeInputs: Array.from(document.querySelectorAll('input[name="mode"]')),
  webrtcSection: document.getElementById("webrtc-section"),
  webrtcToggle: document.getElementById("webrtc-toggle"),
  webrtcHint: document.getElementById("webrtc-hint"),

  whitelistSection: document.getElementById("whitelist-section"),
  whitelistInput: document.getElementById("whitelist-domain"),
  whitelistAdd: document.getElementById("whitelist-add"),
  whitelistAddCurrent: document.getElementById("whitelist-add-current"),
  whitelistList: document.getElementById("whitelist-list"),
  whitelistEmpty: document.getElementById("whitelist-empty"),

  startBtn: document.getElementById("start-btn"),
  stopBtn: document.getElementById("stop-btn"),
  diagnosticsBtn: document.getElementById("diagnostics-btn"),
  authorizedBtn: document.getElementById("authorized-btn"),
};

let lastState = null;

function render(s) {
  lastState = s;

  // Status pill
  els.statusPill.dataset.status = s.status;
  els.statusPill.textContent = STATUS_LABELS[s.status] || s.status;

  // Details
  els.socksPort.textContent = s.socksPort ?? "—";
  if (s.errorMessage) {
    els.errorRow.hidden = false;
    els.errorMessage.textContent = s.errorMessage;
  } else {
    els.errorRow.hidden = true;
  }

  // Mode radio
  for (const input of els.modeInputs) {
    input.checked = input.value === s.mode;
  }

  // WebRTC section: hidden in "all" mode (auto-off), shown otherwise
  if (s.mode === "all") {
    els.webrtcSection.hidden = true;
  } else {
    els.webrtcSection.hidden = false;
    els.webrtcToggle.checked = s.webrtcDisabled === true;
    els.webrtcHint.textContent = s.webrtcDisabled
      ? "WebRTC is OFF for all sites."
      : "WebRTC is left at Firefox's default.";
  }

  // Whitelist section: only in "whitelist" mode
  els.whitelistSection.hidden = s.mode !== "whitelist";
  renderWhitelist(s.whitelist || []);

  // Actions
  els.startBtn.disabled = s.status === "starting" || s.status === "ready";
  els.stopBtn.disabled = s.status !== "ready" && s.status !== "starting";
}

function renderWhitelist(list) {
  els.whitelistList.replaceChildren();
  if (list.length === 0) {
    els.whitelistEmpty.hidden = false;
    return;
  }
  els.whitelistEmpty.hidden = true;
  for (const domain of list) {
    const li = document.createElement("li");
    const span = document.createElement("span");
    span.textContent = domain;
    const btn = document.createElement("button");
    btn.className = "remove";
    btn.type = "button";
    btn.title = `Remove ${domain}`;
    btn.textContent = "✕";
    btn.addEventListener("click", () =>
      browser.runtime.sendMessage({ type: "whitelist-remove", domain })
    );
    li.append(span, btn);
    els.whitelistList.append(li);
  }
}

// ---------- Live state subscription ----------

const port = browser.runtime.connect({ name: "popup" });
port.onMessage.addListener((msg) => {
  if (msg && msg.type === "state") render(msg.state);
});

// Fallback fetch in case the open races the first state push.
browser.runtime
  .sendMessage({ type: "get-state" })
  .then((s) => s && render(s))
  .catch(() => {});

// ---------- Event wiring ----------

for (const input of els.modeInputs) {
  input.addEventListener("change", () => {
    if (!input.checked) return;
    browser.runtime.sendMessage({ type: "set-mode", mode: input.value });
  });
}

els.webrtcToggle.addEventListener("change", () => {
  browser.runtime.sendMessage({
    type: "set-webrtc",
    value: els.webrtcToggle.checked,
  });
});

async function submitWhitelistInput() {
  const v = els.whitelistInput.value.trim();
  if (!v) return;
  const res = await browser.runtime.sendMessage({ type: "whitelist-add", domain: v });
  if (res && res.ok) {
    els.whitelistInput.value = "";
    els.whitelistInput.classList.remove("invalid");
  } else {
    els.whitelistInput.classList.add("invalid");
    els.whitelistInput.focus();
    els.whitelistInput.select();
  }
}

els.whitelistAdd.addEventListener("click", submitWhitelistInput);
els.whitelistInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") submitWhitelistInput();
  els.whitelistInput.classList.remove("invalid");
});

els.whitelistAddCurrent.addEventListener("click", () =>
  browser.runtime.sendMessage({ type: "whitelist-add-current" })
);

els.startBtn.addEventListener("click", () =>
  browser.runtime.sendMessage({ type: "start-tor" })
);
els.stopBtn.addEventListener("click", () =>
  browser.runtime.sendMessage({ type: "stop-tor" })
);

els.diagnosticsBtn.addEventListener("click", () => {
  browser.tabs.create({ url: browser.runtime.getURL("diagnostics.html") });
  window.close();
});

els.authorizedBtn.addEventListener("click", () => {
  browser.tabs.create({ url: browser.runtime.getURL("authorized.html") });
  window.close();
});
