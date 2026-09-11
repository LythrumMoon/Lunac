# Lunac Release Build Pipeline
# 1. Download PaddleOCR-json for offline OCR
# 2. Build Tauri (Rust + frontend)
# 3. Copy outputs to release staging directory
# 4. Compile NSIS installer

param(
  [switch]$SkipInstaller  # Skip NSIS compilation (for dev/testing)
)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$appDir = Join-Path $root "app"
$releaseDir = Join-Path $root "release"
$stagingDir = Join-Path $releaseDir "Lunac"
$targetDir = Join-Path $appDir "src-tauri\target\release"

# ── Step 1: Download PaddleOCR-json ─────────────────────────────
Write-Host "`n===== Step 1: Download PaddleOCR-json =====" -ForegroundColor Cyan
& "$PSScriptRoot\download-paddle-ocr.ps1" -ReleaseDir $stagingDir
if ($LASTEXITCODE -ne 0) {
  Write-Warning "PaddleOCR-json download failed, continuing without it..."
}

# ── Step 2: Build Tauri ─────────────────────────────────────────
Write-Host "`n===== Step 2: Build Tauri =====" -ForegroundColor Cyan
Push-Location $appDir
try {
  bun run tauri:build
  if ($LASTEXITCODE -ne 0) { throw "Tauri build failed" }
} finally {
  Pop-Location
}

# ── Step 3: Copy outputs to staging ─────────────────────────────
Write-Host "`n===== Step 3: Copy to staging =====" -ForegroundColor Cyan
if (-not (Test-Path $stagingDir)) {
  New-Item -ItemType Directory -Path $stagingDir -Force | Out-Null
}

Copy-Item "$targetDir\lunac.exe" -Destination $stagingDir -Force
Write-Host "  lunac.exe ✓"

$cliSource = Join-Path $targetDir "cli.exe"
if (Test-Path $cliSource) {
  Copy-Item $cliSource -Destination $stagingDir -Force
  Write-Host "  cli.exe ✓"
}

$wv2Source = Join-Path $targetDir "WebView2Loader.dll"
if (Test-Path $wv2Source) {
  Copy-Item $wv2Source -Destination $stagingDir -Force
  Write-Host "  WebView2Loader.dll ✓"
}

# ── Step 4: Compile NSIS installer ──────────────────────────────
if (-not $SkipInstaller) {
  Write-Host "`n===== Step 4: Compile NSIS Installer =====" -ForegroundColor Cyan
  $nsisFile = Join-Path $releaseDir "lunac-installer.nsi"
  $makensis = Get-Command makensis -ErrorAction SilentlyContinue
  if (-not $makensis) {
    # Common NSIS install paths
    $nsisPaths = @(
      "${env:ProgramFiles}\NSIS\makensis.exe",
      "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
      "C:\Program Files\NSIS\makensis.exe",
      "C:\Program Files (x86)\NSIS\makensis.exe"
    )
    $makensis = $nsisPaths | Where-Object { Test-Path $_ } | Select-Object -First 1
  }

  if ($makensis) {
    & $makensis $nsisFile
    if ($LASTEXITCODE -eq 0) {
      $setupExe = Get-ChildItem $releaseDir "Lunac-*-Setup.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
      Write-Host "`nInstaller created: $($setupExe.FullName)" -ForegroundColor Green
    } else {
      Write-Error "NSIS compilation failed"
    }
  } else {
    Write-Warning "makensis not found. Install NSIS to compile the installer."
    Write-Warning "You can manually run: makensis $nsisFile"
  }
}

Write-Host "`n===== Build Complete =====" -ForegroundColor Green
Write-Host "Staging: $stagingDir"
Write-Host "Ready for packaging."