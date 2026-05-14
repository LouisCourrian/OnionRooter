# OnionRouter — build pipeline (Windows)
#
# Builds the Rust companion in release mode, packages the Firefox
# extension as an .xpi, and (if NSIS is installed) produces a
# single-file installer.
#
# Output goes to a LOCAL path on C: by default ($env:LOCALAPPDATA\
# OnionRouter-build\), not into the repo. This avoids two headaches:
#
#   1. Windows Defender / SmartScreen heuristics treat executables
#      written to SMB shares as untrusted, blocking reads silently
#      from non-Explorer apps (browsers, file pickers, etc.). Building
#      to local disk avoids the entire class of "access denied on a
#      file that clearly exists" issues.
#
#   2. Repo doesn't get polluted with multi-MB build artefacts and
#      doesn't have to .gitignore them.
#
# Override via:   .\build.ps1 -OutputDir D:\some\other\path
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -SkipInstaller
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -DebugBuild
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1 -OutputDir C:\Out

[CmdletBinding()]
param(
    [switch] $SkipInstaller,
    [switch] $SkipCompanion,
    [switch] $DebugBuild,
    [string] $OutputDir
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

# ----- Output dir (local C: by default, override-able) -----
if (-not $OutputDir) {
    $OutputDir = Join-Path $env:LOCALAPPDATA "OnionRouter-build"
}
if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir | Out-Null
}
$OutputDir = (Resolve-Path $OutputDir).Path

# Sanity check: warn if the user is building to a network path despite
# the default being local. (UNC paths and drive letters from `net use`
# both show up as PSDrive type "FileSystem" with a non-fixed root, but
# the cleanest detection is Win32_LogicalDisk DriveType.)
try {
    $Root = [System.IO.Path]::GetPathRoot($OutputDir)
    $DriveLetter = $Root.TrimEnd('\').TrimEnd(':')
    if ($DriveLetter -and $DriveLetter.Length -eq 1) {
        $Drive = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='${DriveLetter}:'" -ErrorAction SilentlyContinue
        if ($Drive -and $Drive.DriveType -eq 4) {
            Write-Warning "OutputDir is on a network drive ($Root). Windows Defender and SmartScreen may silently block reads of unsigned .exe files written there. Prefer a local drive."
        }
    } elseif ($Root.StartsWith('\\')) {
        Write-Warning "OutputDir is a UNC path ($Root). Same caveat -- prefer a local drive."
    }
} catch {
    # Best-effort warning only; never fail the build for this.
}

Write-Host "Artefacts will land in: $OutputDir" -ForegroundColor Cyan

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
