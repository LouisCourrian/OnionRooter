# addons.mozilla.org listing copy

Paste these into the AMO Developer Hub. Keep them in sync with releases.

> **Important:** the extension needs the **companion** app installed to work.
> Make sure the listing description says so prominently (AMO reviewers and users
> need to know it talks to a native application).

---

## Summary (max 250 chars)

Visit .onion sites in Firefox without Tor Browser. A companion app downloads,
verifies and manages Tor for you; the extension handles routing, modes,
whitelist, DNS-leak and WebRTC protection. Requires the OnionRouter companion.

## Description

OnionRouter lets you open .onion addresses in regular Firefox — Tor is
downloaded, verified and launched automatically in the background.

Requires the free OnionRouter companion app (Windows installer / Debian
package): https://github.com/LouisCourrian/OnionRooter/releases/latest

Features
• Automatic .onion routing — Tor starts on demand.
• Three modes: onion-only, everything-via-Tor, or a custom whitelist.
• No leaks: DNS is forced through Tor; optional WebRTC kill-switch.
• Managed Tor: official Tor Expert Bundle, SHA-256 + PGP verified, auto-updated.
• Reuses a running Tor Browser / system Tor instead of launching a second one.
• Private (client-auth) .onion services: store the access key, encrypted with
  your OS keystore or a passphrase. Keys never leave the companion.
• Diagnostics page and a Tor-routed update check.

Open source (MIT): https://github.com/LouisCourrian/OnionRooter

How it works
The extension talks to a small native companion over Native Messaging. The
companion downloads/verifies/launches Tor (or reuses one), and the extension
routes traffic through it. The extension cannot run Tor by itself — that's why
the companion is required.

Privacy
No analytics, no tracking, no data collection. The companion only contacts the
Tor Project (to fetch Tor) and GitHub over Tor (to check for companion updates).

## What's new (1.0.0)

First stable release. Independent companion/extension updates with a
compatibility check, a Tor-routed "companion update available" prompt, client
authorization for private .onion services, brand icons, and a diagnostics page.
