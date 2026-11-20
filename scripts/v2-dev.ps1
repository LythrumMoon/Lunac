# scripts/v2-dev.ps1 -- Lunac v2 (Flutter) one-stop dev entry.
# ASCII only: PowerShell 5.1 decodes BOM-less UTF-8 as GBK.
#
# Usage (from anywhere, repo-relative):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Check
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Build
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Run
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Launch
#   ... add -Sandbox when running inside the Trae sandbox (it blocks %LOCALAPPDATA%\.dartServer)
#
# -Launch skips the Flutter toolchain entirely and starts the existing debug exe, so it is
# the fast path for "just open the window". For iterative work keep `-Run` alive instead of
# restarting it: press r (hot reload), R (hot restart), q (quit) in its terminal.
#
# Rules:
#   - DEBUG builds only. Release builds are forbidden during the architecture refactor
#     (docs/flutter-migration.md section 6).
#   - No absolute machine paths: everything is derived from this script's location.

[CmdletBinding()]
param(
    [switch]$Check,   # flutter analyze + flutter test
    [switch]$Build,   # flutter build <desktop> --debug
    [switch]$Run,     # flutter run -d <desktop> --debug
    [switch]$Launch,  # start the existing debug build directly (no compile)
    [switch]$Sandbox  # redirect LOCALAPPDATA into <repo>\temp\dart-home for sandboxed runs
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$appDir = Join-Path $repoRoot "app-flutter"

if (-not (Test-Path $appDir)) {
    throw "app-flutter not found: $appDir"
}

# Desktop device name: Windows first, Linux as the second acceptance target.
$device = if ($env:OS -eq "Windows_NT") { "windows" } else { "linux" }

# -Launch does not call the Flutter tool, so it must not require it on PATH.
$needsFlutter = $Check -or $Build -or $Run
if ($needsFlutter -and -not (Get-Command flutter -ErrorAction SilentlyContinue)) {
    throw "flutter not found on PATH. Add <flutter-sdk>\bin to PATH first."
}

# Default action when no switch is given: run.
if (-not ($Check -or $Build -or $Run -or $Launch)) {
    $Run = $true
}

if ($Sandbox) {
    $env:LOCALAPPDATA = Join-Path $repoRoot "temp\dart-home\LocalAppData"
    New-Item -ItemType Directory -Force -Path $env:LOCALAPPDATA | Out-Null
    Write-Host "[v2-dev] sandbox mode: LOCALAPPDATA=$env:LOCALAPPDATA" -ForegroundColor DarkGray
}

function Invoke-Step([string]$label, [scriptblock]$body) {
    Write-Host "[v2-dev] $label" -ForegroundColor Cyan
    & $body
    if ($LASTEXITCODE -ne 0) {
        throw "$label failed with exit code $LASTEXITCODE"
    }
}

Push-Location $appDir
try {
    if ($Check) {
        Invoke-Step "flutter analyze" { flutter analyze }
        Invoke-Step "flutter test" { flutter test }
    }
    if ($Build) {
        Invoke-Step "flutter build $device --debug" { flutter build $device --debug }
    }
    if ($Launch) {
        $exe = if ($device -eq "windows") {
            Join-Path $appDir "build\windows\x64\runner\Debug\lunac.exe"
        } else {
            Join-Path $appDir "build/linux/x64/debug/bundle/lunac"
        }
        if (-not (Test-Path -LiteralPath $exe)) {
            throw "debug build not found: $exe`nRun first: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\v2-dev.ps1 -Build"
        }
        Write-Host "[v2-dev] launch (no build): $exe" -ForegroundColor Cyan
        # Invoke directly instead of Start-Process: the app keeps its console attached, so
        # debug logs (VM service / tray status) stay visible, and it survives this script
        # exiting. PowerShell returns here immediately for a GUI subsystem exe.
        Push-Location (Split-Path -Parent $exe)
        try {
            & $exe
        } finally {
            Pop-Location
        }
    }
    if ($Run) {
        Invoke-Step "flutter run -d $device --debug" { flutter run -d $device --debug }
    }
} finally {
    Pop-Location
}
