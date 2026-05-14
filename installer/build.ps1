# OnionRouter -- build pipeline (Windows)
#
# Builds the Rust companion in release mode, packages the Firefox
# extension as an .xpi, and (if NSIS is installed) produces a
# single-file installer.
#
# Output goes to $RepoRoot\dist\ -- next to the source code, gitignored.
#
# ONE-TIME SETUP (per machine):
#   Run installer\windows\setup-defender-exclusion.ps1 as administrator.
#   This adds dist\ to Defender's exclusion list so unsigned executables
#   built there are readable by every Windows tool (browser file pickers,
#   PowerShell, Explorer copy, etc.). Without this, Defender's
#   reputation-based protection blocks reads of fresh NSIS .exe files
#   built onto SMB shares with no visible error in Protection History.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -SkipInstaller
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -DebugBuild
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -OutputDir D:\elsewhere
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -SkipExclusionCheck

[CmdletBinding()]
param(
    [switch] $SkipInstaller,
    [switch] $SkipCompanion,
    [switch] $DebugBuild,
    [string] $OutputDir,
    [switch] $SkipExclusionCheck
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "..")).Path

# ----- Read version from extension manifest (single source of truth) -----
$ManifestPath = Join-Path $RepoRoot "extension\manifest.json"
$Manifest = Get-Content -Raw -Encoding utf8 $ManifestPath | ConvertFrom-Json
$Version  = $Manifest.version
if (-not $Version) { throw "Could not read version from $ManifestPath" }
Write-Host "Building OnionRouter $Version" -ForegroundColor Cyan

# ----- Output dir (next to source by default) -----
if (-not $OutputDir) {
    $OutputDir = Join-Path $RepoRoot "dist"
}
if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir | Out-Null
}
$OutputDir = (Resolve-Path $OutputDir).Path
Write-Host "Output: $OutputDir" -ForegroundColor Cyan

# ----- Pre-flight: Defender exclusion check -----
# Only matters on Windows + only matters when OutputDir is on a network
# drive (where Defender's reputation-based protection silently blocks
# reads of fresh unsigned executables). On a regular C:\ drive Defender
# is not a problem.
function Test-NetworkDrive {
    param([string] $Path)
    try {
        $root = [System.IO.Path]::GetPathRoot($Path)
        if ($root.StartsWith('\\')) { return $true }
        $letter = $root.TrimEnd('\').TrimEnd(':')
        if ($letter -and $letter.Length -eq 1) {
            $drive = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='${letter}:'" -ErrorAction SilentlyContinue
            return ($drive -and $drive.DriveType -eq 4)
        }
    } catch { }
    return $false
}

function Test-DefenderExclusion {
    param([string] $Path)
    try {
        $exclusions = (Get-MpPreference -ErrorAction Stop).ExclusionPath
    } catch {
        # No Defender (third-party AV) -- can't check, assume OK.
        return $true
    }
    if (-not $exclusions) { return $false }
    $target = [System.IO.Path]::GetFullPath($Path).TrimEnd('\').ToLowerInvariant()
    foreach ($e in $exclusions) {
        if ([string]::IsNullOrWhiteSpace($e)) { continue }
        $excl = [System.IO.Path]::GetFullPath($e).TrimEnd('\').ToLowerInvariant()
        if ($target -eq $excl -or $target.StartsWith($excl + '\')) {
            return $true
        }
    }
    return $false
}

if (-not $SkipExclusionCheck -and (Test-NetworkDrive -Path $OutputDir)) {
    if (-not (Test-DefenderExclusion -Path $OutputDir)) {
        Write-Warning @"
OutputDir is on a network drive AND it is NOT in Defender's exclusion list.

Without an exclusion, Windows Defender will silently block reads of the
unsigned .exe installer this build produces -- you will see the file in
Get-ChildItem but every open() will fail with Access Denied, including
the browser file picker for GitHub releases.

Fix: run this script ONCE as administrator:

  installer\windows\setup-defender-exclusion.ps1

Then re-run the build. Or, if you really want to ignore the warning:

  build.cmd --skip-exclusion-check

(Build will continue in 3 seconds...)
"@
        Start-Sleep -Seconds 3
    }
}

# ----- 1. Companion ------------------------------------------------------
if (-not $SkipCompanion) {
    Write-Host "`n[1/3] Compiling Rust companion..." -ForegroundColor Yellow
    $CargoArgs = @("build", "--manifest-path", (Join-Path $RepoRoot "companion\Cargo.toml"))
    if (-not $DebugBuild) { $CargoArgs += "--release" }
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

$Profile = if ($DebugBuild) { "debug" } else { "release" }
$CompanionExe = Join-Path $RepoRoot "companion\target\$Profile\onionrouter-companion.exe"
if (-not (Test-Path $CompanionExe)) {
    throw "Companion binary not found at $CompanionExe"
}

# ----- 2. XPI ------------------------------------------------------------
Write-Host "`n[2/3] Packaging extension as XPI..." -ForegroundColor Yellow
$XpiPath = Join-Path $OutputDir "onionrouter-$Version.xpi"
$ZipPath = Join-Path $OutputDir "onionrouter-$Version.zip"
Remove-Item -Force -ErrorAction SilentlyContinue $XpiPath, $ZipPath

# Compress-Archive on extension/* puts files at the archive root,
# which is exactly what Firefox expects in an XPI (manifest.json at root).
$ExtensionFiles = Get-ChildItem -Path (Join-Path $RepoRoot "extension") -Force
Compress-Archive -Path $ExtensionFiles.FullName -DestinationPath $ZipPath -CompressionLevel Optimal
Move-Item -Force $ZipPath $XpiPath
Write-Host "  -> $XpiPath" -ForegroundColor Green

# ----- 3. NSIS installer -------------------------------------------------
if ($SkipInstaller) {
    Write-Host "`n[3/3] Skipping NSIS installer (--SkipInstaller)" -ForegroundColor DarkGray
    return
}

Write-Host "`n[3/3] Building NSIS installer..." -ForegroundColor Yellow
$MakeNsis = Get-Command makensis -ErrorAction SilentlyContinue
if (-not $MakeNsis) {
    # Try the standard install path on Windows.
    $Candidate = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
    if (Test-Path $Candidate) { $MakeNsis = Get-Command $Candidate }
}
if (-not $MakeNsis) {
    Write-Warning @"
NSIS not found. The XPI was built, but the installer was NOT.
Install NSIS from https://nsis.sourceforge.io/Download and re-run.
"@
    return
}

$NsiScript = Join-Path $ScriptDir "windows\onionrouter.nsi"
& $MakeNsis.Source `
    "/DAPP_VERSION=$Version" `
    "/DREPO_ROOT=$RepoRoot" `
    "/DOUTPUT_DIR=$OutputDir" `
    $NsiScript
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }

$InstallerPath = Join-Path $OutputDir "OnionRouter-Setup-$Version.exe"
Write-Host "`nDone." -ForegroundColor Green
Write-Host "  XPI:       $XpiPath"
Write-Host "  Installer: $InstallerPath"
