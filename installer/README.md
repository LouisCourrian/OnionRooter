# Installers

For development, the Native Messaging host (the Rust companion) must be
registered with Firefox so the extension can spawn it via
`browser.runtime.connectNative("com.onionrouter.companion")`.

A real, end-user installer is Phase 4. The scripts here are for development
on each platform.

## Layout

```
installer/
├── com.onionrouter.companion.json.template   Native Messaging host manifest
├── windows/
│   └── register-dev.ps1                      Per-user registration via HKCU
├── linux/
│   └── register-dev.sh                       Per-user registration via ~/.mozilla
└── macos/                                     (placeholder — Phase 4)
```

## Windows

```powershell
# from repo root
cd companion ; cargo build ; cd ..
.\installer\windows\register-dev.ps1
```

This:

1. Renders the template with the absolute path of the companion `.exe`.
2. Writes `HKCU:\Software\Mozilla\NativeMessagingHosts\com.onionrouter.companion`
   pointing at the manifest.

## Linux

```bash
cd companion && cargo build && cd ..
./installer/linux/register-dev.sh
```

This writes `~/.mozilla/native-messaging-hosts/com.onionrouter.companion.json`.

## macOS

Phase 4. The manifest location is
`~/Library/Application Support/Mozilla/NativeMessagingHosts/com.onionrouter.companion.json`.

## After registration

In Firefox:

1. Open `about:debugging#/runtime/this-firefox`.
2. Click **Load Temporary Add-on**.
3. Select `extension/manifest.json`.
4. Click the OnionRouter toolbar icon → **Start Tor**.

The first launch triggers a ~50 MB download of the official Tor Expert Bundle.
