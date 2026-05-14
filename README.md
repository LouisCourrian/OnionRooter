# OnionRouter

> Visit `.onion` sites in Firefox without installing Tor Browser. A Rust companion downloads, SHA-256-verifies and runs the official Tor in the background; the extension handles routing, DNS leaks and WebRTC.

## Status

🚧 **MVP** — Phases 1–3 implemented and validated end-to-end (.onion routing,
three modes, whitelist, WebRTC handling, external-Tor reuse via Control Port).
Phase 4 in progress: native Windows installer is done, AMO-signed XPI is the
next step. Not yet on addons.mozilla.org.

See [CAHIER_DES_CHARGES.md](CAHIER_DES_CHARGES.md) for the full specification (French).

## Structure du monorepo

```
onionrouter/
├── companion/      Compagnon Rust (Native Messaging, gestion de Tor)
├── extension/      Extension Firefox (Manifest V3)
├── installer/      Installeurs Windows / Linux / macOS
├── docs/           Documentation technique
└── CAHIER_DES_CHARGES.md
```

## Développement

### Compagnon Rust

```powershell
cd companion
cargo build
cargo run
```

### Extension Firefox

Charger temporairement dans Firefox :

1. Ouvrir `about:debugging#/runtime/this-firefox`
2. Cliquer sur « Charger un module complémentaire temporaire »
3. Sélectionner `extension/manifest.json`

## Phases

- **Phase 1 — MVP** : Native Messaging + téléchargement Tor + détection `.onion` + routage SOCKS5
- **Phase 2 — Robustesse** : détection de Tor existant via Control Port, gestion des erreurs
- **Phase 3 — Modes avancés** : « Tout via Tor », whitelist, persistance, WebRTC
- **Phase 4 — Distribution** : installeurs natifs, publication AMO

## Licence

À définir.
