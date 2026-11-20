# scripts/build-release-v2.ps1 -- build the Lunac v2 (Flutter) release installer.
#
# Usage (from anywhere):
#   powershell -ExecutionPolicy Bypass -File scripts\build-release-v2.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\build-release-v2.ps1 -Version 1.0.1
#   powershell -ExecutionPolicy Bypass -File scripts\build-release-v2.ps1 -NoBuild   # restage existing outputs
#
# Steps:
#   1. Pre-flight (flutter / cargo / makensis / 7z) + version from app-flutter\pubspec.yaml
#   2. Stop running lunac.exe / agent.exe
#   3. Build core-agent release -> agent.exe (always rebuilt: the user asked for a fresh agent)
#   4. flutter build windows --release (CMake calls rust\build.ps1 -Profile Release)
#   5. Stage release\LunacV2\ (bundle + fresh agent.exe + skills\tools\Modules templates)
#   6. Run makensis -> release\Lunac-<version>-Setup.exe, then verify the package
#
# ASCII only: PowerShell 5.1 decodes BOM-less UTF-8 as GBK.

param(
    [string]$Version = "",
    [switch]$NoBuild,
    # Allow cargo / CMake warnings. By default any "warning:" fails the build so
    # the 0-warning baseline stays enforced.
    [switch]$AllowWarnings
)

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$AppFlutter = Join-Path $Root "app-flutter"
$RustDir = Join-Path $AppFlutter "rust"
$CoreAgent = Join-Path $Root "core-agent"
$RunnerRelease = Join-Path $AppFlutter "build\windows\x64\runner\Release"
$ReleaseDir = Join-Path $Root "release"
$Stage = Join-Path $ReleaseDir "LunacV2"
$Nsi = Join-Path $Root "scripts\lunac-v2-installer.nsi"

# rustc needs the repo-local mingw toolchain on PATH (same as rust\build.ps1).
$mingwBin = Join-Path $Root "mingw64\bin"
if (Test-Path $mingwBin) {
    $env:PATH = $mingwBin + ";" + $env:PATH
}

function Invoke-CheckedBuild(
    [string]$Label,
    [string]$WorkDir,
    [string]$Exe,
    [string[]]$CmdArgs
) {
    $log = Join-Path ([IO.Path]::GetTempPath()) ("lunac-build-" + [guid]::NewGuid().ToString("N") + ".log")
    $quoted = @()
    foreach ($arg in $CmdArgs) {
        if ($arg -match '\s') { $quoted += ('"' + $arg + '"') } else { $quoted += $arg }
    }
    $cmdLine = '"{0}" {1} > "{2}" 2>&1' -f $Exe, ($quoted -join ' '), $log
    Push-Location $WorkDir
    try {
        cmd /c $cmdLine
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    if (Test-Path $log) {
        Get-Content $log -Encoding UTF8 | ForEach-Object { Write-Host $_ }
    }
    if ($code -ne 0) {
        Write-Host "  command: $cmdLine" -ForegroundColor DarkGray
        Write-Host "  log kept for diagnosis: $log" -ForegroundColor DarkGray
        throw "$Label failed (exit $code)"
    }
    $warnings = @(Select-String -Path $log -Pattern 'warning\s*:' -AllMatches -ErrorAction SilentlyContinue)
    Remove-Item $log -Force -ErrorAction SilentlyContinue
    if ($warnings.Count -gt 0 -and -not $AllowWarnings) {
        throw "$Label produced $($warnings.Count) warning line(s); fix them or pass -AllowWarnings."
    }
}

function Find-Tool([string[]]$Paths, [string]$Command) {
    $found = $Paths | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $found) {
        $cmd = Get-Command $Command -ErrorAction SilentlyContinue
        if ($cmd) { $found = $cmd.Source }
    }
    return $found
}

$Makensis = Find-Tool @(
    "$env:ProgramFiles\NSIS\makensis.exe",
    "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
) "makensis"

$SevenZip = Find-Tool @(
    "$env:ProgramFiles\7-Zip\7z.exe",
    "${env:ProgramFiles(x86)}\7-Zip\7z.exe"
) "7z.exe"

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Lunac v2 Release Build (Flutter)"       -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# ── 1. Pre-flight ────────────────────────────────────────────────────
Write-Host "[1/6] Pre-flight checks..." -ForegroundColor Yellow
if (-not (Get-Command flutter -ErrorAction SilentlyContinue)) {
    throw "flutter not found on PATH. Add <flutter-sdk>\bin to PATH first."
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo not found on PATH."
}
# Full paths: cmd /c with a quoted bare "flutter" does not append .bat / .exe.
$FlutterExe = (Get-Command flutter).Source
$CargoExe = (Get-Command cargo).Source
if (-not $Makensis) { throw "makensis not found (install NSIS or add it to PATH)." }
if (-not (Test-Path $Nsi)) { throw "NSI script not found: $Nsi" }
if (-not (Test-Path (Join-Path $CoreAgent "Cargo.toml"))) { throw "core-agent not found: $CoreAgent" }
if (-not (Test-Path (Join-Path $AppFlutter "pubspec.yaml"))) { throw "app-flutter\pubspec.yaml not found" }

# Version: pubspec is the single truth source for the Flutter runner (Runner.rc).
$pubspec = Join-Path $AppFlutter "pubspec.yaml"
$pubText = [IO.File]::ReadAllText($pubspec)
$m = [regex]::Match($pubText, '(?m)^version:\s*([0-9]+\.[0-9]+\.[0-9]+)(?:\+([0-9]+))?')
if (-not $m.Success) { throw "Cannot read 'version: x.y.z+N' from $pubspec" }
$pubVersion = $m.Groups[1].Value
$buildNo = if ($m.Groups[2].Success) { $m.Groups[2].Value } else { "1" }

if ($Version) {
    if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
        throw "Version must look like x.y.z, got: $Version"
    }
    if ($Version -ne $pubVersion) {
        Write-Host "  Version: $pubVersion -> $Version (pubspec.yaml)" -ForegroundColor DarkGray
        $newLine = "version: $Version+$buildNo"
        $pubText = [regex]::Replace($pubText, '(?m)^version:\s*.+$', $newLine)
        [IO.File]::WriteAllText($pubspec, $pubText, (New-Object System.Text.UTF8Encoding($false)))
        $pubVersion = $Version
    }
} else {
    Write-Host "  Version: $pubVersion (from pubspec.yaml; pass -Version to override)" -ForegroundColor DarkGray
}
$Version = $pubVersion
Write-Host "  makensis: $Makensis" -ForegroundColor DarkGray
Write-Host "  7z      : $(if ($SevenZip) { $SevenZip } else { '(not found, package check skipped)' })" -ForegroundColor DarkGray
Write-Host ""

# ── 2. Stop running processes ────────────────────────────────────────
Write-Host "[2/6] Stopping running lunac / agent..." -ForegroundColor Yellow
$killed = $false
foreach ($name in @("lunac", "agent")) {
    $proc = Get-Process -Name $name -ErrorAction SilentlyContinue
    if ($proc) {
        # taskkill writes to stderr when the process is already gone; do not let
        # $ErrorActionPreference=Stop turn that into a fatal error.
        $prevEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        taskkill /F /IM "$name.exe" 2>&1 | Out-Null
        $ErrorActionPreference = $prevEap
        Write-Host "  Killed $name.exe" -ForegroundColor DarkGray
        $killed = $true
    }
}
if (-not $killed) { Write-Host "  (no running processes)" -ForegroundColor DarkGray }
Write-Host ""

if (-not $NoBuild) {
    # ── 3. agent.exe (always fresh) ──────────────────────────────────
    Write-Host "[3/6] Building agent.exe (cargo build --release, core-agent)..." -ForegroundColor Yellow
    Invoke-CheckedBuild "agent.exe build" $CoreAgent $CargoExe @("build", "--release")
    Write-Host ""

    # ── 4. Flutter release bundle ────────────────────────────────────
    Write-Host "[4/6] flutter build windows --release..." -ForegroundColor Yellow
    Invoke-CheckedBuild "flutter build" $AppFlutter $FlutterExe @("build", "windows", "--release")
    Write-Host ""
} else {
    Write-Host "[3/6] -NoBuild: reusing core-agent\target\release\agent.exe" -ForegroundColor DarkGray
    Write-Host "[4/6] -NoBuild: reusing $RunnerRelease" -ForegroundColor DarkGray
    Write-Host ""
}

if (-not (Test-Path $RunnerRelease)) {
    throw "Flutter release bundle not found: $RunnerRelease"
}

# ── 5. Stage release\LunacV2 ────────────────────────────────────────
Write-Host "[5/6] Staging release\LunacV2..." -ForegroundColor Yellow
if (Test-Path $Stage) {
    Remove-Item -Recurse -Force $Stage
    Write-Host "  Cleared stale staging dir" -ForegroundColor DarkGray
}
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

# The whole Flutter bundle (exe + DLLs + data\). Never ship dev-run leftovers.
Copy-Item (Join-Path $RunnerRelease "*") $Stage -Recurse -Force
foreach ($junk in @("config", "ModuleData", "temp")) {
    $path = Join-Path $Stage $junk
    if (Test-Path $path) { Remove-Item -Recurse -Force $path }
}
Get-ChildItem $Stage -Filter "*.pdb" -File -Recurse -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue

# Fresh agent.exe wins over any copy the Flutter build left behind.
$builtAgent = Join-Path $CoreAgent "target\release\agent.exe"
if (-not (Test-Path $builtAgent)) {
    throw "agent.exe not found: $builtAgent (run without -NoBuild)"
}
Copy-Item $builtAgent (Join-Path $Stage "agent.exe") -Force

# Skills / tools / Modules templates (same contract as the v1 installer).
$tplRoot = Join-Path $Root "agent-templates"
foreach ($pair in @(@("skills", "skills"), @("tools", "tools"), @("modules", "Modules"))) {
    $src = Join-Path $tplRoot $pair[0]
    if (-not (Test-Path $src)) { throw "agent-templates\$($pair[0]) not found: $src" }
    $dst = Join-Path $Stage $pair[1]
    New-Item -ItemType Directory -Force -Path $dst | Out-Null
    Copy-Item (Join-Path $src "*") $dst -Recurse -Force
}

# -- Curated index (docs/todo.md 3-28.9 item 1) ----------------------
# Read-only mirror + manifest. Every entry's sha256 is computed from the mirror here,
# never hand-written. The settings UI compares it with what is installed:
# installed / outdated / missing.
$mirror = Join-Path $Stage "agent-templates\skills"
New-Item -ItemType Directory -Force -Path $mirror | Out-Null
Copy-Item (Join-Path $tplRoot "skills\*") $mirror -Recurse -Force

$curatedPath = Join-Path $Stage "skills\curated.json"
if (-not (Test-Path $curatedPath)) { throw "skills\curated.json missing from templates" }
$curated = Get-Content $curatedPath -Raw -Encoding UTF8 | ConvertFrom-Json
foreach ($entry in $curated.entries) {
    $skillFile = Join-Path $mirror ($entry.key + "\SKILL.md")
    if (-not (Test-Path $skillFile)) {
        throw "curated entry '$($entry.key)' has no SKILL.md in the template mirror"
    }
    $entry.sha256 = (Get-FileHash $skillFile -Algorithm SHA256).Hash.ToLower()
}
$curatedJson = $curated | ConvertTo-Json -Depth 6
[IO.File]::WriteAllText($curatedPath, $curatedJson, (New-Object Text.UTF8Encoding($false)))
Write-Host "  OK: curated.json ($($curated.entries.Count) entries, hashes pinned)" -ForegroundColor Green

$required = @(
    "lunac.exe",
    "agent.exe",
    "lunac_core.dll",
    "flutter_windows.dll",
    "WebView2Loader.dll",
    "data\icudtl.dat"
)
foreach ($rel in $required) {
    $path = Join-Path $Stage $rel
    if (-not (Test-Path $path)) { throw "staged bundle is missing: $rel" }
}
Write-Host "  OK: lunac.exe + agent.exe + lunac_core.dll + data\" -ForegroundColor Green
Write-Host ""

# ── 6. makensis + package verification ─────────────────────────────
Write-Host "[6/6] makensis..." -ForegroundColor Yellow

# Rewrite PRODUCT_VERSION (BOM-safe: the NSI is ASCII, so write UTF-8 without BOM).
$nsiBytes = [IO.File]::ReadAllBytes($Nsi)
$nsiBom = 0
while ($nsiBom * 3 + 2 -lt $nsiBytes.Length -and
       $nsiBytes[$nsiBom * 3] -eq 0xEF -and $nsiBytes[$nsiBom * 3 + 1] -eq 0xBB -and $nsiBytes[$nsiBom * 3 + 2] -eq 0xBF) { $nsiBom++ }
$nsiText = [Text.Encoding]::UTF8.GetString($nsiBytes, $nsiBom * 3, $nsiBytes.Length - $nsiBom * 3)
$newDefine = '!define PRODUCT_VERSION "' + $Version + '"'
$nsiText = [regex]::Replace($nsiText, '!define PRODUCT_VERSION "[^"]*"', $newDefine)
if ($nsiText -notmatch [regex]::Escape($newDefine)) {
    throw "Failed to update PRODUCT_VERSION in $Nsi"
}
[IO.File]::WriteAllText($Nsi, $nsiText, (New-Object System.Text.UTF8Encoding($false)))

$SetupPath = Join-Path $ReleaseDir "Lunac-$Version-Setup.exe"
if (Test-Path $SetupPath) {
    try {
        Remove-Item $SetupPath -Force -ErrorAction Stop
    } catch {
        throw "Cannot remove previous installer (locked by antivirus or another process): $SetupPath`n$($_.Exception.Message)"
    }
}

Push-Location $ReleaseDir
try {
    $output = & $Makensis $Nsi 2>&1
    Write-Host $output -ForegroundColor DarkGray
    if ($LASTEXITCODE -ne 0) { throw "makensis failed (exit $LASTEXITCODE)" }
} finally { Pop-Location }

if (-not (Test-Path $SetupPath)) {
    throw "Setup.exe not produced: $SetupPath"
}

if ($SevenZip) {
    $packed = ((& $SevenZip l $SetupPath 2>&1) -join "`n") -replace '\s+', ''
    if ($packed -match 'cli\.exe') {
        throw "Installer contains cli.exe (upstream CLI, must not be redistributed)"
    }
    foreach ($needle in @('agent\.exe', 'lunac\.exe', 'lunac_core\.dll', 'flutter_windows\.dll', 'icudtl\.dat')) {
        if ($packed -notmatch $needle) {
            throw "Installer is missing $($needle -replace '\\', '') -- package check failed"
        }
    }
    Write-Host "  Package OK: contains agent.exe; no cli.exe" -ForegroundColor Green
} else {
    Write-Host "  [SKIP] 7z.exe not found; package content check skipped" -ForegroundColor Yellow
}

$setupMb = [math]::Round((Get-Item $SetupPath).Length / 1MB, 1)
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Build complete"                          -ForegroundColor Cyan
Write-Host "  $SetupPath"                             -ForegroundColor White
Write-Host "  Size : $setupMb MB"                     -ForegroundColor White
Write-Host "========================================" -ForegroundColor Cyan
