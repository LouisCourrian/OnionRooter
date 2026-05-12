// OnionRouter popup — opens a runtime port to the background script so
// state updates stream in live without polling.

"use strict";

const STATUS_LABELS = {
  disconnected: "Inactive",
  starting: "Starting Tor…",
  ready: "Tor active",
  error: "Error",
};

const els = {
  statusSection: document.getElementById("status"),
  statusLabel: document.getElementById("status-label"),
  socksPort: document.getElementById("socks-port"),
  errorRow: document.getElementById("error-row"),
  errorMessage: document.getElementById("error-message"),
  startBtn: document.getElementById("start-btn"),
  stopBtn: document.getElementById("stop-btn"),
};

function render(state) {
  els.statusSection.dataset.status = state.status;
  els.statusLabel.textContent = STATUS_LABELS[state.status] || state.status;
  els.socksPort.textContent = state.socksPort ?? "—";

  if (state.errorMessage) {
    els.errorRow.hidden = false;
    els.errorMessage.textContent = state.errorMessage;
  } else {
    els.errorRow.hidden = true;
  }

  els.startBtn.disabled = state.status === "starting" || state.status === "ready";
  els.stopBtn.disabled = state.status !== "ready" && state.status !== "starting";
}

const port = browser.runtime.connect({ name: "popup" });

port.onMessage.addListener((msg) => {
  if (msg && msg.type === "state") render(msg.state);
});

els.startBtn.addEventListener("click", () => {
  browser.runtime.sendMessage({ type: "start-tor" });
});

els.stopBtn.addEventListener("click", () => {
  browser.runtime.sendMessage({ type: "stop-tor" });
});

// Fallback initial fetch in case the port message races the open.
browser.runtime
  .sendMessage({ type: "get-state" })
  .then((state) => state && render(state))
  .catch(() => {});
