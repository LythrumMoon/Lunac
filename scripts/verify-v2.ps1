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

function Assert-InstallerLayout {
    $iss = Join-Path $Root "scripts\lunac-v2-installer.iss"
    $nsi = Join-Path $Root "scripts\lunac-v2-installer.nsi"
    if (-not (Test-Path $iss)) { throw "Inno Setup script not found: $iss" }
    if (-not (Test-Path $nsi)) { throw "NSIS fallback script not found: $nsi" }

    $issText = [IO.File]::ReadAllText($iss)
    $issChecks = @(
        @{ Pattern = 'WizardStyle=modern dark'; Why = "Inno dark wizard (themed buttons)" },
        @{ Pattern = 'AutoStartCheckBox'; Why = "auto-start checkbox on the directory page" },
        @{ Pattern = 'DesktopIconCheckBox'; Why = "desktop shortcut checkbox on the directory page" },
        @{ Pattern = 'WizardForm\.DirEdit\.Parent'; Why = "checkboxes parented to the directory page" },
        @{ Pattern = 'postinstall'; Why = "finish-page run option" },
        @{ Pattern = 'WizardImageFile=installer-welcome\.bmp'; Why = "themed welcome art" },
        @{ Pattern = 'WizardSmallImageFile=installer-small\.bmp'; Why = "themed small art" }
    )
    foreach ($check in $issChecks) {
        if ($issText -notmatch $check.Pattern) {
            throw "installer layout missing: $($check.Why)"
        }
    }

    $nsiText = [IO.File]::ReadAllText($nsi)
    if ($nsiText -notmatch 'Page custom InstallOptionsPage') {
        throw "NSIS fallback is missing the combined options page"
    }
    if ($nsiText -match 'MUI_PAGE_DIRECTORY') {
        throw "MUI_PAGE_DIRECTORY must not be used: folder and auto-start are one page"
    }
    if ($nsiText -notmatch 'MUI_FINISHPAGE_RUN') {
        throw "NSIS fallback is missing the finish-page run option"
    }

    $wrapper = Join-Path $Root "scripts\lunac-setup-wrapper.nsi"
    if (-not (Test-Path $wrapper)) { throw "setup wrapper not found: $wrapper" }
    $wrapperText = [IO.File]::ReadAllText($wrapper)
    foreach ($needle in @('INNER_SETUP', '"/S"', '/SILENT', '/DIR=', 'SetErrorLevel')) {
        if ($wrapperText -notmatch [regex]::Escape($needle)) {
            throw "setup wrapper is missing $needle (old /S auto-update compatibility)"
        }
    }

    foreach ($art in @("installer-welcome.bmp", "installer-header.bmp", "installer-small.bmp")) {
        $path = Join-Path $Root "scripts\$art"
        if (-not (Test-Path $path)) { throw "installer art missing: $path (run scripts\make-installer-art.ps1)" }
    }
    Write-Host "  installer layout OK (Inno dark wizard + combined options page + finish run option)" -ForegroundColor Green
}

$flutter = Require-Command "flutter"
$cargo = Require-Command "cargo"

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Lunac v2 verification" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

Assert-InstallerLayout

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
