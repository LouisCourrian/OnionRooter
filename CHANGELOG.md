# Changelog

From 1.0.0 onward the companion and the extension are versioned independently
(`companion-vX.Y.Z`, `ext-vX.Y.Z`); earlier `v0.x` releases shipped both together.

## 1.0.0 — first stable

- First public stable release. The extension is published on
  [addons.mozilla.org](https://addons.mozilla.org/firefox/addon/onionrouter/);
  the companion ships as a signed Windows installer and Debian package.
- **Independent release pipelines** for companion and extension.
- **Native-messaging protocol version** + "companion out of date" prompt, so the
  two halves stay compatible across independent updates.
- **Companion update check over Tor** — checks GitHub for a newer companion
  without leaking your IP.
- Windows installer points to the AMO listing instead of bundling an unsigned
  XPI.
- Brand icons (toolbar states + app/tray icon), product README + architecture
  diagram.

## 0.5.0 — onion client authorization

- Access restricted (client-auth) v3 `.onion` services: store an x25519 key per
  service, encrypted with the OS keystore or a passphrase; injected into Tor via
  the control port. On-demand unlock page; keys never touch the browser profile.

## 0.4.0

- First-launch welcome notification.
- SAFECOOKIE control-port auth — reuse a running Tor Browser.
- Diagnostic fixes (report the real Tor version; correct source label).

## 0.3.0 — Tor auto-update

- The companion discovers the latest Tor version and installs it after verifying
  the Tor Project's **PGP signature** on the checksums, with the pinned version
  as a fallback.

## 0.2.x

- `0.2.4` — fix Tor download (re-pin to a version still hosted by the Tor
  Project).
- `0.2.3` — diagnostic page (extension + companion `diagnostic` action).
- `0.2.2` — Debian/Ubuntu companion package; GPG-signed release artifacts.
- `0.2.0`–`0.2.1` — routing modes (onion / all / whitelist), whitelist
  management, WebRTC handling, Windows installer + tray companion.

## 0.1.x

- Initial MVP: `.onion` detection, SOCKS5 routing, Tor download + SHA-256
  verification, on-demand launch, status icon.
