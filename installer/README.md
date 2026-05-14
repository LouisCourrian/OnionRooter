# Installer & distribution

Two flows are documented here:

- **Automated release via GitHub Actions** (recommended) — push a `v*`
  tag, CI builds and publishes the release for you.
- **Local build** — for iteration, manual testing, or one-off
  experiments.

## Layout

```
installer/
├── build.ps1                                      Orchestrator: companion + XPI + NSIS
├── com.onionrouter.companion.json.template        Native Messaging host template
├── windows/
│   ├── onionrouter.nsi                            NSIS script (driven by build.ps1)
│   └── register-dev.ps1                           Dev-only HKCU registration
├── linux/
│   └── register-dev.sh                            Dev-only ~/.mozilla registration
└── macos/                                          (Phase 4 -- TBD)

.github/workflows/
├── release.yml                                    Triggered on v* tags
└── ci.yml                                         Triggered on push / PR
```

## Automated release (recommended)

```cmd
cd <repo>
git tag v0.1.1
git push origin v0.1.1
```

This triggers `.github/workflows/release.yml` on a Windows runner. It:

1. Checks out the tagged commit.
2. Installs Rust + NSIS.
3. Runs the test suite (`cargo test`).
4. Builds the companion in release mode.
5. Packages the extension as `onionrouter-<ver>.xpi`.
6. Builds `OnionRouter-Setup-<ver>.exe`.
7. Computes SHA-256 sums for both artefacts.
8. Creates a GitHub Release pre-populated with the artefacts and
   auto-generated changelog notes since the previous tag.

No local build required. No Defender shenanigans. Reproducible.

## Development setup (Windows)

For iterating on the extension or companion before tagging a release.

```cmd
cd companion
cargo build
cd ..
powershell -ExecutionPolicy Bypass -File installer\windows\register-dev.ps1
```

Then in Firefox: `about:debugging#/runtime/this-firefox` -> **Load
Temporary Add-on** -> pick `extension/manifest.json`.

The temporary add-on disappears at Firefox restart -- that's expected
for the dev flow.

## Development setup (Linux)

```bash
cd companion && cargo build && cd ..
./installer/linux/register-dev.sh
```

Then load the extension the same way via `about:debugging`.

## Local distribution build (Windows)

> Prefer the GitHub Actions flow above. Build locally only when you
> need to test the installer behaviour itself or work offline.

### Prerequisites

- Rust toolchain (`cargo`, `rustup`).
- NSIS 3.x -- download from <https://nsis.sourceforge.io/Download>.
  Default install path (`C:\Program Files (x86)\NSIS\`) is auto-detected.
- PowerShell 5+ (ships with Windows 10/11).
- **Clone the repo to a local drive** (not a SMB share / mapped network
  drive). Windows Defender's reputation-based protection silently blocks
  reads of unsigned executables freshly created on network drives, even
  for the file picker that uploads them to GitHub. Local NTFS sidesteps
  this. CI is unaffected (the runners have no such constraint).

### Build

```cmd
build.cmd
```

or, in PowerShell:

```powershell
.\installer\build.ps1
```

Produces, in `dist/`:

- `onionrouter-<ver>.xpi`
- `OnionRouter-Setup-<ver>.exe`

`dist/` is gitignored.

Flags:

- `--skip-installer` -- XPI only, no NSIS step.
- `--debug` -- use `cargo build` (faster) instead of `cargo build --release`.
- `-OutputDir <path>` -- override the output location.

### What the installer does

When the end user runs `OnionRouter-Setup-<ver>.exe`:

1. Per-user install -- no admin prompt, files go under
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
2. **Submit a New Add-on** -- upload `dist\onionrouter-<ver>.xpi`.
3. Choose **"On this site"** distribution.
4. Fill in the listing (description, screenshots, support URLs).
5. AMO automatic + human review (hours to days for a first listing).
6. Once approved, AMO publishes the signed `.xpi` at a stable URL.
   Users install via the standard "Add to Firefox" button.

### B. Self-distributed (unlisted)

For niche / pre-release builds you want to control yourself:

1. AMO developer account, same as above.
2. **Submit a New Add-on** -- upload the `.xpi`.
3. Choose **"On your own"** distribution.
4. AMO signs automatically (no human review for self-distributed).
5. Download the signed `.xpi`.
6. Drop it into `dist\` overwriting the unsigned one, and re-trigger
   the release workflow so the bundled XPI is the signed version.
7. Ship the installer wherever (your site, GitHub Releases, etc).

### Automating the sign step

The `web-ext` CLI can upload + retrieve the signed `.xpi` in one go,
given AMO API credentials:

```bash
npx web-ext sign \
  --source-dir=extension \
  --artifacts-dir=dist \
  --channel=unlisted \
  --api-key=user:XXXX \
  --api-secret=YYYY
```

API keys come from <https://addons.mozilla.org/developers/addon/api/key/>.
This step would slot into the release workflow as a job step gated on
secrets being configured -- left as a Phase 4 follow-up.

## Uninstall

Two ways:

- **Settings -> Apps -> OnionRouter -> Uninstall** (Windows GUI).
- `%LOCALAPPDATA%\OnionRouter\uninstall.exe` (or `/S` for silent).

Both remove the binary, manifest, HKCU registry entries, and the Tor
download cache (`%LOCALAPPDATA%\OnionRouter\tor\`).

## Linux / macOS distribution

Not in this sprint -- planned for the next Phase 4 batch (`.deb` for
Debian/Ubuntu, `.pkg` for macOS).
