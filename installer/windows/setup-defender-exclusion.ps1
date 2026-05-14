# OnionRouter -- one-shot Defender exclusion for the build output folder.
#
# Why this exists:
#   Windows Defender's reputation-based protection (SmartScreen) silently
#   blocks reads of unsigned executables freshly created on network shares
#   (SMB/UNC). This means a perfectly valid NSIS installer built into the
#   repo's dist/ folder ends up unreadable by the very tools that need to
#   read it (browser file pickers, PowerShell Copy-Item, etc.) -- with no
#   visible entry in Protection History.
#
#   Adding the build folder to Defender's exclusion list disables the
#   reputation check for that path only. Defender keeps protecting
#   everything else normally.
#
# This is standard practice -- Rust, .NET, Node and Java toolchains all
# recommend excluding their build output folders for both performance
# and false-positive reasons.
#
# Run ONCE per machine, as administrator:
#
#   1. Right-click PowerShell -> Run as administrator
#   2. cd Z:\programmation\OnionRooter
#   3. .\installer\windows\setup-defender-exclusion.ps1
#
# Or in one shot from a non-elevated prompt (UAC prompt will appear):
#
#   powershell -Command "Start-Process powershell -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','.\installer\windows\setup-defender-exclusion.ps1' -Verb RunAs"

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "..\..")).Path
$DistDir   = Join-Path $RepoRoot "dist"

# ---- Admin check ----
$identity  = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
$isAdmin   = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    Write-Error @"
This script must be run as administrator.

Open PowerShell as administrator (right-click PowerShell -> Run as administrator),
then re-run this script.

Or auto-elevate from a normal prompt:

  Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','$PSCommandPath'
"@
    exit 1
}

# ---- Make sure the dist folder exists so Defender accepts the exclusion ----
if (-not (Test-Path $DistDir)) {
    New-Item -ItemType Directory -Path $DistDir -Force | Out-Null
    Write-Host "Created $DistDir" -ForegroundColor DarkGray
}

# ---- Defender availability check ----
$mp = Get-Command Get-MpPreference -ErrorAction SilentlyContinue
if (-not $mp) {
    Write-Error @"
The Defender PowerShell module (Get-MpPreference / Add-MpPreference) is not
available on this machine. This usually means either:

  - You're on a Windows edition without Defender (rare), or
  - A third-party AV has replaced Defender (Avast, McAfee, etc.).

In the second case, add the following path to your AV's exclusion list
manually via its own settings UI:

  $DistDir
"@
    exit 1
}

$current = (Get-MpPreference).ExclusionPath

function Path-IsAlreadyExcluded {
    param([string] $Path, [string[]] $Exclusions)
    if (-not $Exclusions) { return $false }
    foreach ($e in $Exclusions) {
        if ([string]::IsNullOrWhiteSpace($e)) { continue }
        if ([System.IO.Path]::GetFullPath($e).TrimEnd('\') -eq [System.IO.Path]::GetFullPath($Path).TrimEnd('\')) {
            return $true
        }
    }
    return $false
}

if (Path-IsAlreadyExcluded -Path $DistDir -Exclusions $current) {
    Write-Host "Defender exclusion already present:" -ForegroundColor Green
    Write-Host "  $DistDir"
    exit 0
}

# ---- Add exclusion ----
try {
    Add-MpPreference -ExclusionPath $DistDir
} catch {
    Write-Error @"
Failed to add Defender exclusion: $($_.Exception.Message)

Possible causes:
  - Tamper protection is enabled on Defender. Disable it temporarily in
    Windows Security -> Virus & threat protection settings -> Tamper
    protection, then re-run this script and re-enable afterwards.
  - Group policy is locking Defender exclusions (typical on corporate
    laptops). Ask your IT admin.
"@
    exit 1
}

Write-Host ""
Write-Host "Defender exclusion added:" -ForegroundColor Green
Write-Host "  $DistDir"
Write-Host ""
Write-Host "From now on, you can build the installer with:"
Write-Host "  build.cmd"
Write-Host "and the artefacts will land in $DistDir without Defender getting in the way."
Write-Host ""
Write-Host "To undo:" -ForegroundColor DarkGray
Write-Host "  Remove-MpPreference -ExclusionPath '$DistDir'" -ForegroundColor DarkGray
