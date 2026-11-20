# Build the Rust side as a cdylib.
# Usage: powershell -File build.ps1 [-Profile debug|release]
# Default = debug (dev loop). Release is only used by scripts\build-release-v2.ps1
# (2026-11-19 user asked for a release installer; see docs\flutter-migration.md).
# ASCII only: PowerShell 5.1 decodes BOM-less UTF-8 as GBK.
param(
    [string]$Profile = "debug"
)

$ErrorActionPreference = "Stop"

# Repo root = app-flutter/rust/../..
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
# rustc needs the mingw tools on PATH on this machine; keep the path repo-relative.
$mingwBin = Join-Path $repoRoot "mingw64\bin"
if (Test-Path $mingwBin) {
    $env:PATH = $mingwBin + ";" + $env:PATH
}

$cargoArgs = @("build")
if ($Profile -eq "release" -or $Profile -eq "Release") {
    $cargoArgs += "--release"
}

Push-Location $PSScriptRoot
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($cargoArgs -join ' ') failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
