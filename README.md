# OnionRouter

[![CI](https://github.com/LouisCourrian/OnionRooter/actions/workflows/ci.yml/badge.svg)](https://github.com/LouisCourrian/OnionRooter/actions/workflows/ci.yml)
[![Release](https://github.com/LouisCourrian/OnionRooter/actions/workflows/release.yml/badge.svg)](https://github.com/LouisCourrian/OnionRooter/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Visit `.onion` sites in Firefox without installing Tor Browser. A Rust Native
Messaging companion downloads, SHA-256-verifies, launches, and reuses Tor; the
Firefox extension handles routing, DNS leak prevention, modes, whitelist, and
WebRTC settings.

## Status

Version: `0.5.1`.

Phases 1-3 are implemented and validated end-to-end:

- `.onion` routing through Tor.
- Three modes: onion-only, all traffic, whitelist.
- Whitelist persistence and "add current site".
- WebRTC handling.
- External Tor reuse through Control Port verification.
- Windows tray companion.

Phase 4 is in progress:

- Windows installer: implemented.
- Debian/Ubuntu companion package: implemented for `amd64`.
- AMO-signed XPI: available outside this repo for now.
- macOS package and diagnostic page: still pending.

See [CAHIER_DES_CHARGES.md](CAHIER_DES_CHARGES.md) for the functional scope and
[docs/TECHNICAL.md](docs/TECHNICAL.md) for the technical architecture.

## Monorepo structure

```text
onionrouter/
|-- companion/      Rust companion: Native Messaging, Tor management
|-- extension/      Firefox extension: Manifest V3
|-- installer/      Windows and Linux packaging
|-- docs/           Technical documentation
`-- CAHIER_DES_CHARGES.md
```

## Development

### Rust companion

```powershell
cd companion
cargo build
cargo test
```

### Firefox extension

Load temporarily in Firefox:

1. Open `about:debugging#/runtime/this-firefox`.
2. Click "Load Temporary Add-on".
3. Select `extension/manifest.json`.

### Windows dev registration

```powershell
cargo build --manifest-path companion\Cargo.toml
powershell -ExecutionPolicy Bypass -File installer\windows\register-dev.ps1
```

### Linux dev registration

```bash
cargo build --manifest-path companion/Cargo.toml
./installer/linux/register-dev.sh
```

## Packaging

Windows:

```powershell
.\installer\build.ps1
```

Debian/Ubuntu:

```bash
bash installer/linux/build-deb.sh
```

Release builds are handled by `.github/workflows/release.yml` when pushing a
`v*` tag.

## Contributing

This is a personal project; pull requests are not accepted at this time.

Feedback is welcome through GitHub issues. Security-sensitive reports should be
filed privately through GitHub's vulnerability reporting flow once the security
policy is configured.

## License

[MIT](LICENSE) (c) Louis COURRIAN.
