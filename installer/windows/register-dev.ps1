# Dev-time registration of the OnionRouter Native Messaging host on Windows.
#
# Usage (from project root, PowerShell):
#   .\installer\windows\register-dev.ps1
#
# This is for development only. A real installer (Phase 4) will place the
# manifest under %ProgramFiles%\OnionRouter and write the registry key with
# elevated privileges. Here, we register under HKCU so no admin rights are
# needed and the install is per-user.

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$CompanionExe = Join-Path $RepoRoot "companion\target\release\onionrouter-companion.exe"
$CompanionExeDebug = Join-Path $RepoRoot "companion\target\debug\onionrouter-companion.exe"
$Template = Join-Path $RepoRoot "installer\com.onionrouter.companion.json.template"
$Manifest = Join-Path $RepoRoot "installer\com.onionrouter.companion.json"

if (Test-Path $CompanionExe) {
    $ExePath = $CompanionExe
} elseif (Test-Path $CompanionExeDebug) {
    $ExePath = $CompanionExeDebug
    Write-Host "Using debug build at $ExePath"
} else {
    Write-Error "Companion binary not found. Run 'cargo build' in .\companion first."
    exit 1
}

# Generate manifest with absolute path baked in.
$content = Get-Content -Raw -Encoding utf8 $Template
$content = $content.Replace("{{COMPANION_PATH}}", $ExePath.Replace("\", "\\"))
Set-Content -Path $Manifest -Value $content -Encoding utf8

# Register under HKCU so no admin rights are needed.
$RegPath = "HKCU:\Software\Mozilla\NativeMessagingHosts\com.onionrouter.companion"
if (-not (Test-Path $RegPath)) {
    New-Item -Path $RegPath -Force | Out-Null
}
Set-ItemProperty -Path $RegPath -Name "(Default)" -Value $Manifest

Write-Host "Registered OnionRouter companion:"
Write-Host "  Manifest: $Manifest"
Write-Host "  Binary  : $ExePath"
Write-Host "  Registry: $RegPath"
Write-Host ""
Write-Host "Next: load the extension via about:debugging in Firefox."
