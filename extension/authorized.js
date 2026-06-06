// OnionRouter — Authorized services management page.
//
// Talks to the companion (via the background) for all client-auth operations:
// list, add, remove, generate key pair, set/unlock/lock the passphrase vault.

"use strict";

const els = {
  pill: document.getElementById("status-pill"),
  vaultBody: document.getElementById("vault-body"),

  paste: document.getElementById("paste"),
  detect: document.getElementById("detect"),
  addr: document.getElementById("addr"),
  label: document.getElementById("label"),
  tierOs: document.getElementById("tier-os"),
  tierPp: document.getElementById("tier-pp"),
  osHint: document.getElementById("os-hint"),
  ppHint: document.getElementById("pp-hint"),
  addBtn: document.getElementById("add-btn"),
  genBtn: document.getElementById("gen-btn"),
  genbox: document.getElementById("genbox"),
  genpub: document.getElementById("genpub"),
  gencopy: document.getElementById("gencopy"),
  addMsg: document.getElementById("add-msg"),

  rows: document.getElementById("rows"),
  empty: document.getElementById("empty"),
};

let state = { entries: [], os_available: false, vault_exists: false, unlocked: false };

/** Send an auth command to the companion through the background. */
async function auth(action, extra = {}) {
  return browser.runtime.sendMessage({ type: "auth", payload: { action, ...extra } });
}

function setMsg(el, text, kind) {
  el.textContent = text || "";
  el.className = "msg" + (kind ? " " + kind : "");
}

async function load() {
  const r = await auth("auth-list");
  if (r && r.ok && r.data) {
    state = r.data;
    render();
  } else {
    setMsg(els.addMsg, (r && r.error) || "Could not reach the companion.", "err");
  }
}

function render() {
  // Status pill
  if (!state.vault_exists) {
    els.pill.textContent = "No passphrase vault";
    els.pill.dataset.s = "none";
  } else if (state.unlocked) {
    els.pill.textContent = "Unlocked";
    els.pill.dataset.s = "unlocked";
  } else {
    els.pill.textContent = "Locked";
    els.pill.dataset.s = "locked";
  }

  renderVault();
  renderTierOptions();
  renderRows();
}

function renderVault() {
  els.vaultBody.replaceChildren();
  if (!state.vault_exists) {
    const note = document.createElement("p");
    note.className = "vault-note";
    note.textContent =
      "Set a passphrase to enable the passphrase-protected tier. Max-privacy: keys are encrypted with a passphrase only you know.";
    const row = document.createElement("div");
    row.className = "vault-row";
    const inp = document.createElement("input");
    inp.type = "password";
    inp.placeholder = "Choose a passphrase";
    const btn = document.createElement("button");
    btn.className = "primary";
    btn.textContent = "Set passphrase";
    btn.addEventListener("click", async () => {
      if (!inp.value) return;
      btn.disabled = true;
      const r = await auth("auth-set-passphrase", { passphrase: inp.value });
      btn.disabled = false;
      if (r && r.ok) load();
      else setMsg(els.addMsg, (r && r.error) || "Failed.", "err");
    });
    row.append(inp, btn);
    els.vaultBody.append(note, row);
  } else if (!state.unlocked) {
    const row = document.createElement("div");
    row.className = "vault-row";
    const inp = document.createElement("input");
    inp.type = "password";
    inp.placeholder = "Passphrase";
    const btn = document.createElement("button");
    btn.className = "primary";
    btn.textContent = "Unlock";
    const doUnlock = async () => {
      if (!inp.value) return;
      btn.disabled = true;
      const r = await auth("auth-unlock", { passphrase: inp.value });
      btn.disabled = false;
      if (r && r.ok) load();
      else setMsg(els.addMsg, (r && r.error) || "Wrong passphrase.", "err");
    };
    btn.addEventListener("click", doUnlock);
    inp.addEventListener("keydown", (e) => { if (e.key === "Enter") doUnlock(); });
    row.append(inp, btn);
    els.vaultBody.append(row);
  } else {
    const ok = document.createElement("span");
    ok.className = "vault-ok";
    ok.textContent = "🔓 Unlocked for this session";
    const btn = document.createElement("button");
    btn.textContent = "Lock";
    btn.style.marginLeft = "12px";
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      const r = await auth("auth-lock");
      btn.disabled = false;
      if (r && r.ok) load();
    });
    const row = document.createElement("div");
    row.className = "vault-row";
    row.append(ok, btn);
    els.vaultBody.append(row);
  }
}

function renderTierOptions() {
  els.tierOs.disabled = !state.os_available;
  els.osHint.textContent = state.os_available ? "(recommended)" : "(Windows only)";
  els.tierPp.disabled = !state.vault_exists;
  els.ppHint.textContent = state.vault_exists
    ? (state.unlocked ? "" : "(unlock first to add)")
    : "(set a passphrase first)";
  // pick a sensible default
  if (els.tierOs.checked && els.tierOs.disabled) els.tierOs.checked = false;
  if (els.tierPp.checked && els.tierPp.disabled) els.tierPp.checked = false;
  if (!els.tierOs.checked && !els.tierPp.checked) {
    if (!els.tierOs.disabled) els.tierOs.checked = true;
    else if (!els.tierPp.disabled) els.tierPp.checked = true;
  }
}

function renderRows() {
  els.rows.replaceChildren();
  const entries = state.entries || [];
  els.empty.hidden = entries.length > 0;
  for (const e of entries) {
    const tr = document.createElement("tr");

    const tdLabel = document.createElement("td");
    tdLabel.className = "lbl";
    tdLabel.textContent = e.label || "(no label)";

    const tdAddr = document.createElement("td");
    tdAddr.className = "addr";
    if (e.onion) tdAddr.textContent = e.onion + ".onion";
    else tdAddr.textContent = "🔒 unlock to view";

    const tdTier = document.createElement("td");
    const badge = document.createElement("span");
    badge.className = "badge";
    badge.textContent = e.tier === "os" ? "OS" : "passphrase";
    tdTier.append(badge);

    const tdRm = document.createElement("td");
    const rm = document.createElement("button");
    rm.className = "rm";
    rm.title = "Remove";
    rm.textContent = "🗑";
    rm.disabled = !e.onion; // need the address (locked passphrase entries hide it)
    rm.addEventListener("click", async () => {
      rm.disabled = true;
      const r = await auth("auth-remove", { onion: e.onion });
      if (r && r.ok) load();
      else { rm.disabled = false; setMsg(els.addMsg, (r && r.error) || "Remove failed.", "err"); }
    });
    tdRm.append(rm);

    tr.append(tdLabel, tdAddr, tdTier, tdRm);
    els.rows.append(tr);
  }
}

// ---------- Add / generate ----------

function selectedTier() {
  if (els.tierOs.checked) return "os";
  if (els.tierPp.checked) return "passphrase";
  return null;
}

els.paste.addEventListener("input", () => {
  const v = els.paste.value.trim();
  if (v.includes(":descriptor:x25519:")) {
    const parts = v.split(":");
    if (parts[0]) els.addr.value = parts[0] + ".onion";
    els.detect.innerHTML = "Detected: <b>full .auth_private line</b> — address auto-filled.";
  } else if (/^[A-Za-z2-7]{52}$/.test(v)) {
    els.detect.innerHTML = "Detected: <b>base32 key</b>.";
  } else if (/^[A-Za-z0-9+/]{43,44}=?$/.test(v)) {
    els.detect.innerHTML = "Detected: <b>base64 key</b>.";
  } else {
    els.detect.innerHTML =
      "Accepts base32, base64, or a full <code>addr:descriptor:x25519:key</code> line — auto-detected.";
  }
});

els.addBtn.addEventListener("click", async () => {
  const key = els.paste.value.trim();
  const tier = selectedTier();
  if (!key) { setMsg(els.addMsg, "Paste a private key first.", "err"); return; }
  if (!tier) { setMsg(els.addMsg, "Choose where to store the key.", "err"); return; }

  els.addBtn.disabled = true;
  setMsg(els.addMsg, "Adding…", "");
  const r = await auth("auth-add", {
    onion: els.addr.value.trim(),
    label: els.label.value.trim(),
    key,
    tier,
  });
  els.addBtn.disabled = false;
  if (r && r.ok) {
    els.paste.value = "";
    els.addr.value = "";
    els.label.value = "";
    els.genbox.hidden = true;
    setMsg(els.addMsg, "Added and loaded into Tor.", "ok");
    load();
  } else {
    setMsg(els.addMsg, (r && r.error) || "Add failed.", "err");
  }
});

els.genBtn.addEventListener("click", async () => {
  els.genBtn.disabled = true;
  const r = await auth("auth-generate");
  els.genBtn.disabled = false;
  if (r && r.ok && r.data) {
    els.paste.value = r.data.private;
    els.paste.dispatchEvent(new Event("input"));
    els.genpub.textContent = r.data.public;
    els.genbox.hidden = false;
    setMsg(els.addMsg, "Key pair generated. Set the address + label, then Add.", "ok");
  } else {
    setMsg(els.addMsg, (r && r.error) || "Generation failed.", "err");
  }
});

els.gencopy.addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(els.genpub.textContent);
    els.gencopy.textContent = "Copied";
    setTimeout(() => (els.gencopy.textContent = "Copy"), 1500);
  } catch {
    /* ignore */
  }
});

load();
