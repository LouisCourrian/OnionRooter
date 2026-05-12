# Installer & distribution

Two distinct flows are documented here:

- **Development setup** — load the extension as a temporary add-on,
  register the companion via PowerShell, iterate fast.
- **Distribution build** — produce a single-file `.exe` installer
  that drops the companion + bundled `.xpi` on an end user's machine
  and registers the Native Messaging host.

## Layout

```
installer/
├── build.ps1                                      Orchestrator: companion + XPI + NSIS
├── build/                                         (gitignored) build artefacts
│   ├── onionrouter-<ver>.xpi
│   └── OnionRouter-Setup-<ver>.exe
├── com.onionrouter.companion.json.template        Native Messaging host template
├── windows/
│   ├── onionrouter.nsi                            NSIS script (driven by build.ps1)
│   └── register-dev.ps1                           Dev-only HKCU registration
├── linux/
│   └── register-dev.sh                            Dev-only ~/.mozilla registration
└── macos/                                          (Phase 4 — TBD)
```

## Development setup (Windows)

```cmd
cd Z:\programmation\OnionRooter\companion
cargo build
cd ..
powershell -ExecutionPolicy Bypass -File installer\windows\register-dev.ps1
```

Then in Firefox: `about:debugging#/runtime/this-firefox` → **Load Temporary
Add-on** → pick `extension/manifest.json`.

The temporary add-on disappears at Firefox restart — that's expected for
the dev flow.

## Development setup (Linux)

```bash
cd companion && cargo build && cd ..
./installer/linux/register-dev.sh
```

Then load the extension the same way via `about:debugging`.

## Building the distribution installer (Windows)

### Prerequisites

- Rust toolchain (`cargo`, `rustup`).
- NSIS 3.x — download from <https://nsis.sourceforge.io/Download>.
  Default install path (`C:\Program Files (x86)\NSIS\`) is auto-detected.
- PowerShell 5+ (ships with Windows 10/11).

### Build

```cmd
build.cmd
```

or, if you prefer PowerShell directly:

```powershell
.\installer\build.ps1
```

This produces:

- `installer\build\onionrouter-<ver>.xpi` — the unsigned extension package.
- `installer\build\OnionRouter-Setup-<ver>.exe` — the installer.

Flags:

- `--skip-installer` — XPI only, no NSIS step.
- `--debug` — use `cargo build` (faster) instead of `cargo build --release`.

### What the installer does

When the end user runs `OnionRouter-Setup-<ver>.exe`:

1. Per-user install — no admin prompt, files go under
   `%LOCALAPPDATA%\OnionRouter\`.
2. Drops `bin\onionrouter-companion.exe`.
3. Generates `com.onionrouter.companion.json` with the absolute path to
   the binary and the extension ID baked in.
4. Writes the registry key
   `HKCU\Software\Mozilla\NativeMessagingHosts\com.onionrouter.companion`
   pointing at that JSON.
5. Places `extension.xpi` alongside, with the on-screen instruction to
   drag it into Firefox.
6. Registers an "Apps & Features" entry so Windows can uninstall the
   companion cleanly (removes binary, manifest, registry, Tor data dir).

## Getting the XPI signed (AMO)

Firefox refuses unsigned extensions in standard releases. To make the
`.xpi` installable for end users, it must be signed by Mozilla.

Two distribution paths:

### A. Listed on addons.mozilla.org (recommended for public release)

1. Create an AMO developer account at <https://addons.mozilla.org/developers/>.
2. **Submit a New Add-on** → upload `installer\build\onionrouter-<ver>.xpi`.
3. Choose **"On this site"** distribution.
4. Fill in the listing (description, screenshots, support URLs).
5. AMO automatic + human review (hours to days for a first listing).
6. Once approved, AMO publishes the signed `.xpi` at a stable URL.
   Users install via the standard "Add to Firefox" button.

### B. Self-distributed (unlisted)

For niche / pre-release builds you want to control yourself:

1. AMO developer account, same as above.
2. **Submit a New Add-on** → upload the `.xpi`.
3. Choose **"On your own"** distribution.
4. AMO signs automatically (no human review for self-distributed).
5. Download the signed `.xpi`.
6. Drop it into `installer\build\` overwriting the unsigned one, and
   rebuild the installer (`build.cmd`) so the bundled XPI is the
   signed version.
7. Ship the installer wherever (your site, GitHub Releases, etc).

### Automating the sign step

The `web-ext` CLI can upload + retrieve the signed `.xpi` in one go,
given AMO API credentials:

```bash
npx web-ext sign \
  --source-dir=extension \
  --artifacts-dir=installer/build \
  --channel=unlisted \
  --api-key=user:XXXX \
  --api-secret=YYYY
```

API keys come from <https://addons.mozilla.org/developers/addon/api/key/>.
This step would slot into `build.ps1` as a `-Sign` switch — left as
Phase 4 follow-up to keep credentials out of the default flow.

## Uninstall

Two ways:

- **Settings → Apps → OnionRouter → Uninstall** (Windows GUI).
- `%LOCALAPPDATA%\OnionRouter\uninstall.exe` (or `/S` for silent).

Both remove the binary, manifest, HKCU registry entries, and the Tor
download cache (`%LOCALAPPDATA%\OnionRouter\tor\`).

## Linux / macOS distribution

Not in this sprint — planned for the next Phase 4 batch (`.deb` for
Debian/Ubuntu, `.pkg` for macOS).
