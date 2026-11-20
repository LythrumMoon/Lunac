# scripts/verify-v2.ps1 -- one-shot verification for the v2 (Flutter) app.
#
# Usage (from anywhere):
#   powershell -ExecutionPolicy Bypass -File scripts\verify-v2.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\verify-v2.ps1 -SkipTests
#   powershell -ExecutionPolicy Bypass -File scripts\verify-v2.ps1 -Full
#
# Default: analyze + flutter tests + cargo check/test + core-agent tests.
# -Full additionally runs scripts\build-release-v2.ps1 (slow; produces the installer).
#
# ASCII only: PowerShell 5.1 decodes BOM-less UTF-8 as GBK.

param(
    [switch]$SkipTests,
    [switch]$Full
)

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$AppFlutter = Join-Path $Root "app-flutter"
$RustDir = Join-Path $AppFlutter "rust"
$CoreAgent = Join-Path $Root "core-agent"

$mingwBin = Join-Path $Root "mingw64\bin"
if (Test-Path $mingwBin) {
    $env:PATH = $mingwBin + ";" + $env:PATH
}

function Require-Command([string]$Name) {
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $cmd) { throw "$Name not found on PATH." }
    return $cmd.Source
}

function Invoke-Step([string]$Name, [string]$WorkDir, [string[]]$Command) {
    Write-Host ""
    Write-Host "== $Name ==" -ForegroundColor Cyan
    Push-Location $WorkDir
    try {
        $exe = $Command[0]
        $cmdArgs = @()
        if ($Command.Length -gt 1) { $cmdArgs = $Command[1..($Command.Length - 1)] }
        & $exe @cmdArgs
        if ($LASTEXITCODE -ne 0) {
            throw "$Name failed (exit $LASTEXITCODE)"
        }
    } finally {
        Pop-Location
    }
}

$flutter = Require-Command "flutter"
$cargo = Require-Command "cargo"

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Lunac v2 verification" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

Invoke-Step "flutter analyze" $AppFlutter @($flutter, "analyze")
if (-not $SkipTests) {
    Invoke-Step "flutter test" $AppFlutter @($flutter, "test")
}
Invoke-Step "lunac_core cargo check (release)" $RustDir @($cargo, "check", "--release")
if (-not $SkipTests) {
    Invoke-Step "lunac_core cargo test (lib)" $RustDir @($cargo, "test", "--lib")
}
if (-not $SkipTests) {
    Invoke-Step "core-agent cargo test" $CoreAgent @($cargo, "test", "--offline")
}

if ($Full) {
    $build = Join-Path $Root "scripts\build-release-v2.ps1"
    Invoke-Step "build-release-v2.ps1" $Root @("powershell", "-ExecutionPolicy", "Bypass", "-File", $build)
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "  v2 verification passed" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
