// OnionRouter — interstitial unlock page.
//
// Reached when the background interceptor redirects a navigation to a
// passphrase-protected .onion that is still locked. On success the companion
// loads the key into Tor and we send the tab back to the original URL.

"use strict";

const params = new URLSearchParams(location.search);
const onion = params.get("onion") || "";
const label = params.get("label") || "";
const returnUrl = params.get("return") || (onion ? `http://${onion}.onion/` : "about:blank");

const els = {
  label: document.getElementById("svc-label"),
  addr: document.getElementById("svc-addr"),
  form: document.getElementById("form"),
  passphrase: document.getElementById("passphrase"),
  submit: document.getElementById("submit"),
  error: document.getElementById("error"),
  cancel: document.getElementById("cancel"),
};

if (label) els.label.textContent = label;
els.addr.textContent = onion ? `${onion}.onion` : "";

function showError(msg) {
  els.error.hidden = false;
  els.error.textContent = msg;
  els.passphrase.classList.add("invalid");
  els.passphrase.focus();
  els.passphrase.select();
}

els.form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const passphrase = els.passphrase.value;
  if (!passphrase) return;

  els.submit.disabled = true;
  els.submit.textContent = "Unlocking…";
  els.error.hidden = true;
  els.passphrase.classList.remove("invalid");

  try {
    const res = await browser.runtime.sendMessage({
      type: "auth",
      payload: { action: "auth-unlock", passphrase },
    });
    if (res && res.ok) {
      location.href = returnUrl;
      return;
    }
    showError((res && res.error) || "Unlock failed.");
  } catch (err) {
    showError(String((err && err.message) || err));
  } finally {
    els.submit.disabled = false;
    els.submit.textContent = "Unlock & continue";
  }
});

els.cancel.addEventListener("click", () => {
  // Go back if we can, otherwise close the tab.
  if (history.length > 1) history.back();
  else window.close();
});
