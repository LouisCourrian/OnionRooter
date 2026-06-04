# Installer & distribution

This document covers the release and local packaging flows for OnionRouter.

## Layout

```text
installer/
|-- build.ps1                                  Windows orchestrator: companion + XPI + NSIS
|-- com.onionrouter.companion.json.template    Native Messaging host template
|-- windows/
|   |-- onionrouter.nsi                        NSIS script
|   `-- register-dev.ps1                       Dev-only HKCU registration
|-- linux/
|   |-- build-deb.sh                           Debian/Ubuntu package builder
|   `-- register-dev.sh                        Dev-only ~/.mozilla registration
`-- macos/                                     Future .pkg packaging

.github/workflows/
|-- release.yml                                Triggered on v* tags
`-- ci.yml                                     Triggered on push / PR
```

## Automated release

Recommended flow:

```cmd
cd <repo>
git tag v0.2.3
git push origin v0.2.3
```

This triggers `.github/workflows/release.yml`.

The Windows job:

1. Checks out the tagged commit.
2. Installs Rust and NSIS.
3. Runs the companion test suite.
4. Builds the companion in release mode.
5. Packages the extension as `onionrouter-<ver>.xpi`.
6. Builds `OnionRouter-Setup-<ver>.exe`.
7. Computes SHA-256 sums for the Windows artefacts.
8. Publishes the XPI, installer, and `SHA256SUMS.txt`.

The Linux job:

1. Checks out the tagged commit.
2. Installs Rust.
3. Builds `onionrouter-companion_<ver>_amd64.deb`.
4. Computes a `.sha256` checksum for the Debian package.
5. Uploads both files to the same GitHub Release.

No local build is required for release publication.

## Development setup - Windows

For iterating on the extension or companion before tagging a release:

```cmd
cd companion
cargo build
cd ..
powershell -ExecutionPolicy Bypass -File installer\windows\register-dev.ps1
```

Then in Firefox:

1. Open `about:debugging#/runtime/this-firefox`.
2. Click "Load Temporary Add-on".
3. Select `extension/manifest.json`.

The temporary add-on disappears when Firefox restarts. That is expected for
the development flow.

## Development setup - Linux

```bash
cd companion && cargo build && cd ..
./installer/linux/register-dev.sh
```

Then load the extension the same way through `about:debugging`.

## Local distribution build - Windows

Prefer the GitHub Actions flow for release builds. Build locally only when you
need to test the installer behavior itself or work offline.

Prerequisites:

- Rust toolchain (`cargo`, `rustup`).
- NSIS 3.x.
- PowerShell 5+.
- A local NTFS clone is recommended for manual Windows artefact handling.

Build:

```cmd
build.cmd
```

or:

```powershell
.\installer\build.ps1
```

Produces, in `dist/`:

- `onionrouter-<ver>.xpi`
- `OnionRouter-Setup-<ver>.exe`

Flags:

- `--skip-installer`: XPI only, no NSIS step.
- `--debug`: use `cargo build` instead of `cargo build --release`.
- `-OutputDir <path>`: override the output location.

## Local distribution build - Debian/Ubuntu

The Debian package installs only the Native Messaging companion and the
Firefox host manifest. The extension itself is distributed separately through
AMO or as a signed XPI.

Prerequisites:

- Rust toolchain (`cargo`, `rustup`).
- Python 3.
- `dpkg-deb`.
- Linux x86_64 / Debian architecture `amd64`.

Build:

```bash
bash installer/linux/build-deb.sh
```

Produces, in `dist/`:

- `onionrouter-companion_<ver>_amd64.deb`

The package installs:

- `/usr/lib/onionrouter/onionrouter-companion`
- `/usr/lib/mozilla/native-messaging-hosts/com.onionrouter.companion.json`
- `/usr/share/doc/onionrouter-companion/`

Install locally:

```bash
sudo apt install ./dist/onionrouter-companion_<ver>_amd64.deb
```

## What the Windows installer does

When the end user runs `OnionRouter-Setup-<ver>.exe`:

1. Installs per-user, without admin prompt, under `%LOCALAPPDATA%\OnionRouter\`.
2. Drops `bin\onionrouter-companion.exe`.
3. Generates `com.onionrouter.companion.json` with the absolute binary path.
4. Writes the Firefox Native Messaging registry key under HKCU.
5. Places `extension.xpi` alongside the companion.
6. Registers an "Apps & Features" uninstall entry.
7. Registers the tray daemon at user login.

## Getting the XPI signed

Firefox refuses unsigned extensions in standard releases. End-user XPI files
must be signed by Mozilla.

Two distribution paths are supported:

- Listed on addons.mozilla.org, recommended for public release.
- Self-distributed / unlisted, useful for controlled pre-releases.

The `web-ext` CLI can automate signing when AMO API credentials are available:

```bash
npx web-ext sign \
  --source-dir=extension \
  --artifacts-dir=dist \
  --channel=unlisted \
  --api-key=user:XXXX \
  --api-secret=YYYY
```

This step is intentionally not wired into CI until release secrets are present.

## Uninstall

Windows:

- Settings -> Apps -> OnionRouter -> Uninstall.
- `%LOCALAPPDATA%\OnionRouter\uninstall.exe`.

Debian/Ubuntu:

```bash
sudo apt remove onionrouter-companion
```

The downloaded Tor cache lives in each user's local data directory and is not
owned by the Debian package.

## Remaining distribution work

- macOS `.pkg` packaging.
- Optional AMO signing automation once API secrets are configured.
