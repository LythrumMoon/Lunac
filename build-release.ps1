# Lunac Release Build Script
# Usage: .\build-release.ps1 [version]
#   .\build-release.ps1           - reads version from package.json
#   .\build-release.ps1 0.6.0     - explicit version
#
# Steps:
#   1. Pre-flight checks (bun / cargo / makensis)
#   2. Kill existing processes
#   3. Build web assets (tsc + vite)
#   4. Build Rust binary (cargo build --release)
#   5. Build cli.exe (bun build --compile)
#   6. Copy binaries to release/Lunac/
#   7. Package VSCode extension (.vsix)
#   8. Stage PaddleOCR-json for offline OCR
#   9. Update NSI version + run makensis → Setup.exe

param(
  [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSCommandPath
$Stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

# ═══════════════════════════════════════════════════════════════════
# 1. Pre-flight checks
# ═══════════════════════════════════════════════════════════════════

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Lunac Release Build"                    -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# ── Version ────────────────────────────────────────────────────────
if (-not $Version) {
  $PkgJsonPath = "$Root\app\package.json"
  if (-not (Test-Path $PkgJsonPath)) {
    throw "package.json not found at $PkgJsonPath"
  }
  $PkgJson = Get-Content $PkgJsonPath -Raw | ConvertFrom-Json
  $Version = $PkgJson.version
  if (-not $Version) {
    throw "Could not read version from package.json"
  }
}
Write-Host "  Version : $Version" -ForegroundColor White
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ── Tool checks ────────────────────────────────────────────────────
Write-Host "[1/9] Pre-flight checks..." -ForegroundColor Yellow

$Checks = @{
  "bun"      = { bun --version 2>&1 | Out-Null; $LASTEXITCODE -eq 0 }
  "cargo"    = { cargo --version 2>&1 | Out-Null; $LASTEXITCODE -eq 0 }
  "makensis" = { Test-Path "C:\Program Files (x86)\NSIS\Bin\makensis.exe" }
}

$AllOk = $true
foreach ($tool in $Checks.Keys) {
  $ok = & $Checks[$tool]
  if ($ok) {
    Write-Host "  [OK]  $tool" -ForegroundColor Green
  } else {
    Write-Host "  [MISS] $tool — install before building" -ForegroundColor Red
    $AllOk = $false
  }
}

# ── Source dirs ────────────────────────────────────────────────────
$RequiredDirs = @(
  "$Root\app",
  "$Root\app\src-tauri",
  "$Root\core",
  "$Root\release"
)
foreach ($dir in $RequiredDirs) {
  if (-not (Test-Path $dir)) {
    Write-Host "  [MISS] $dir" -ForegroundColor Red
    $AllOk = $false
  }
}

if (-not $AllOk) {
  throw "Pre-flight checks failed. Fix missing dependencies before building."
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 2. Kill existing processes (they may hold file locks)
# ═══════════════════════════════════════════════════════════════════

Write-Host "[2/9] Killing running processes..." -ForegroundColor Yellow
$Killed = $false
foreach ($name in @("lunac", "cli")) {
  $proc = Get-Process -Name $name -ErrorAction SilentlyContinue
  if ($proc) {
    taskkill /F /IM "$name.exe" 2>$null | Out-Null
    Write-Host "  Killed $name.exe" -ForegroundColor DarkGray
    $Killed = $true
  }
}
if (-not $Killed) {
  Write-Host "  (no running processes)" -ForegroundColor DarkGray
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 3. Build web assets (TypeScript + Vite)
# ═══════════════════════════════════════════════════════════════════

Write-Host "[3/9] Building web assets (tsc + vite)..." -ForegroundColor Yellow
Push-Location "$Root\app"
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  npm run build
  if ($LASTEXITCODE -ne 0) { throw "Web build failed (exit $LASTEXITCODE)" }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 4. Build Rust release binary (lunac.exe)
# ═══════════════════════════════════════════════════════════════════

Write-Host "[4/9] Building Rust binary (cargo build --release)..." -ForegroundColor Yellow
Push-Location "$Root\app\src-tauri"
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  cargo build --release
  if ($LASTEXITCODE -ne 0) { throw "Rust build failed (exit $LASTEXITCODE)" }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 5. Build cli.exe (Bun standalone binary)
# ═══════════════════════════════════════════════════════════════════

Write-Host "[5/9] Building cli.exe (bun build --compile)..." -ForegroundColor Yellow
Push-Location "$Root\core"
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  bun build --compile entrypoints/cli.tsx --outfile cli.exe
  if ($LASTEXITCODE -ne 0) { throw "cli.exe build failed (exit $LASTEXITCODE)" }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 6. Verify & copy binaries to release/Lunac/
# ═══════════════════════════════════════════════════════════════════

Write-Host "[6/9] Copying binaries to release/Lunac/..." -ForegroundColor Yellow

$ReleaseDir = "$Root\release"
$AppDir     = "$ReleaseDir\Lunac"
New-Item -ItemType Directory $AppDir -Force | Out-Null

$Binaries = @{
  "lunac.exe"           = "$Root\app\src-tauri\target\release\lunac.exe"
  "cli.exe"             = "$Root\core\cli.exe"
  "WebView2Loader.dll"  = "$Root\app\src-tauri\target\release\WebView2Loader.dll"
}

$TotalBinSize = 0
foreach ($name in $Binaries.Keys) {
  $src = $Binaries[$name]
  if (-not (Test-Path $src)) {
    throw "Binary not found: $src"
  }
  Copy-Item $src "$AppDir\$name" -Force
  $size = (Get-Item $src).Length
  $TotalBinSize += $size
  $sizeMb = [math]::Round($size / 1MB, 1)
  Write-Host "  $name  $sizeMb MB" -ForegroundColor DarkGray
}
$totalMb = [math]::Round($TotalBinSize / 1MB, 1)
Write-Host "  Total : $totalMb MB" -ForegroundColor Green
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 7. Package VSCode extension (.vsix) + copy to release/Lunac/
# ═══════════════════════════════════════════════════════════════════
# The Lunac desktop app's "Attach to VSCode" button opens VSCode and
# auto-installs the extension if the .vsix is found next to lunac.exe.
# Users can also install it manually from the install directory.

$VsixSrc = "$Root\vscode-extension"
$VsixOut = "$AppDir\lunac.vsix"

Write-Host "[7/9] VSCode extension (.vsix)..." -ForegroundColor Yellow

if (Test-Path $VsixSrc) {
  try {
    Push-Location $VsixSrc

    # 1) Prefer a pre-built .vsix whose filename embeds the target version.
    #    (Get-ChildItem default order is alphabetical — a stale older .vsix
    #    (e.g. lunac-0.6.0.vsix) sorts first and used to get copied even when
    #    it did not match the app version. Match by version instead.)
    $vsixFiles = Get-ChildItem -Path . -Filter "*.vsix" -File
    $vsix = $vsixFiles |
      Where-Object { $_.Name -match [regex]::Escape($Version) } |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1

    # 2) No version-matched package exists → build one now so the shipped
    #    extension always matches the app version.
    if (-not $vsix) {
      # Install dependencies first (first build or node_modules missing)
      if (-not (Test-Path "node_modules\@vscode\vsce")) {
        Write-Host "  Installing vsce..." -ForegroundColor DarkGray
        npm install --no-audit --no-fund 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
          Write-Host "  npm install failed (exit $LASTEXITCODE)" -ForegroundColor Yellow
        }
      }
      if (Test-Path "node_modules\@vscode\vsce") {
        Write-Host "  Building with vsce package (v$Version)..." -ForegroundColor DarkGray
        # Use & to invoke local node_modules/.bin/vsce directly (avoids npx cache)
        $vsceCmd = Join-Path $VsixSrc "node_modules\.bin\vsce.cmd"
        & $vsceCmd package --allow-missing-repository 2>&1 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
        if ($LASTEXITCODE -eq 0) {
          $vsix = Get-ChildItem -Path . -Filter "*.vsix" -File |
            Where-Object { $_.Name -match [regex]::Escape($Version) } |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1
        } else {
          Write-Host "  vsce package failed (exit $LASTEXITCODE) — manual: cd vscode-extension && npm run package" -ForegroundColor Yellow
        }
      }
    }

    # 3) Copy the chosen package, or fall back to newest with a WARN.
    if ($vsix) {
      Copy-Item $vsix.FullName $VsixOut -Force
      $vsixSize = [math]::Round($vsix.Length / 1KB, 1)
      Write-Host "  Copied $($vsix.Name) → lunac.vsix ($vsixSize KB)" -ForegroundColor Green
    } elseif ($vsixFiles) {
      $fallback = $vsixFiles | Sort-Object LastWriteTime -Descending | Select-Object -First 1
      Write-Host "  WARN: no v$Version .vsix available; copying newest $($fallback.Name)" -ForegroundColor Yellow
      Copy-Item $fallback.FullName $VsixOut -Force
    } else {
      Write-Host "  No .vsix found (extension not packaged)" -ForegroundColor DarkGray
    }
  } finally {
    Pop-Location
  }
} else {
  Write-Host "  vscode-extension/ not found, skipping" -ForegroundColor DarkGray
}

# If we have the .vsix at the destination, verify it
if (Test-Path $VsixOut) {
  Write-Host "  Verified: $VsixOut" -ForegroundColor Green
} else {
  Write-Host "  Skipped (vscode-extension not built — 'Attach to VSCode' will open VSCode without extension auto-install)" -ForegroundColor DarkGray
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 8. Stage PaddleOCR-json (offline OCR engine)
# ═══════════════════════════════════════════════════════════════════
# Priority: 1) copy from local paddle-ocr/  2) download from GitHub
# Always does a fresh copy — no stale/partial copies from previous builds.

Write-Host "[8/9] PaddleOCR-json (offline OCR)..." -ForegroundColor Yellow
$PaddleDir = "$AppDir\paddle-ocr"

# 解压 .7z：优先 7z.exe，其次系统自带 bsdtar（Windows 10 1803+）。
# PowerShell 的 Expand-Archive 不支持 7z，而 PaddleOCR-json 的 Windows 资产是 .7z。
function Expand-SevenZip {
  param([string]$Archive, [string]$Destination)
  $candidates = @(
    "$env:ProgramFiles\7-Zip\7z.exe",
    "${env:ProgramFiles(x86)}\7-Zip\7z.exe"
  )
  $exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $exe) {
    $cmd = Get-Command 7z.exe -ErrorAction SilentlyContinue
    if ($cmd) { $exe = $cmd.Source }
  }
  if ($exe) {
    & $exe x $Archive "-o$Destination" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "7z exited with code $LASTEXITCODE" }
    return
  }
  & tar -xf $Archive -C $Destination
  if ($LASTEXITCODE -ne 0) {
    throw "解压 $Archive 失败：请安装 7-Zip 后重试（tar 返回 $LASTEXITCODE）"
  }
}

# Always start clean — remove any stale/partial copy from previous builds.
# The "Already staged" shortcut was unreliable: if a previous build was
# interrupted after copying the exe but before models/DLLs, the partial
# copy would be skipped on next run, producing a broken release folder.
if (Test-Path $PaddleDir) {
  Remove-Item -Recurse -Force $PaddleDir
  Write-Host "  Removed previous paddle-ocr/ (fresh build)" -ForegroundColor DarkGray
}

# Priority 1: copy from local source (dev repo already has PaddleOCR-json)
$LocalPaddle = "$Root\paddle-ocr\PaddleOCR-json\PaddleOCR-json_v1.4.1"
if (Test-Path (Join-Path $LocalPaddle "PaddleOCR-json.exe")) {
  Write-Host "  Copying from local paddle-ocr/..."
  # Copy directory CONTENTS (not the wrapper folder itself) to produce flat structure:
  #   release/Lunac/paddle-ocr/PaddleOCR-json.exe  (not .../PaddleOCR-json_v1.4.1/...)
  New-Item -ItemType Directory -Path $PaddleDir -Force | Out-Null
  Copy-Item "$LocalPaddle\*" -Destination $PaddleDir -Recurse -Force

  # Verify key files were copied (exe + models config + core runtime DLLs)
  $paddleSize = [math]::Round((Get-ChildItem $PaddleDir -Recurse | Measure-Object Length -Sum).Sum / 1MB, 1)
  $exeExists = Test-Path (Join-Path $PaddleDir "PaddleOCR-json.exe")
  $modelsExist = Test-Path (Join-Path $PaddleDir "models\config_chinese.txt")
  $dllExists = Test-Path (Join-Path $PaddleDir "paddle_inference.dll")
  if ($exeExists -and $modelsExist -and $dllExists) {
    Write-Host "  Copied ($paddleSize MB) — verified OK" -ForegroundColor Green
  } else {
    throw "PaddleOCR-json copy incomplete: exe=$exeExists models=$modelsExist paddle_inference.dll=$dllExists"
  }
} else {
  # Priority 2: download from GitHub releases
  # NOTE: the v1.4.1 Windows asset is a .7z (no .zip); Expand-Archive cannot read it.
  $PaddleVersion = "v1.4.1"
  $PaddleArchive = "PaddleOCR-json_v1.4.1_windows_x64.7z"
  $PaddleUrl = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/$PaddleVersion/$PaddleArchive"
  $TempArchive = "$env:TEMP\$PaddleArchive"
  $TempExtract = "$env:TEMP\paddle-ocr-extract"

  try {
    Write-Host "  Downloading $PaddleVersion from GitHub..."
    Invoke-WebRequest -Uri $PaddleUrl -OutFile $TempArchive -UseBasicParsing
    Write-Host "  Download complete." -ForegroundColor DarkGray

    if (Test-Path $TempExtract) { Remove-Item -Recurse -Force $TempExtract }
    New-Item -ItemType Directory -Path $TempExtract -Force | Out-Null
    Expand-SevenZip -Archive $TempArchive -Destination $TempExtract

    # The archive contains a PaddleOCR-json_v1.4.1/ folder; copy its contents flat
    New-Item -ItemType Directory -Path $PaddleDir -Force | Out-Null
    $innerDir = Get-ChildItem -Path $TempExtract -Directory | Select-Object -First 1
    if ($innerDir) {
      Copy-Item "$($innerDir.FullName)\*" -Destination $PaddleDir -Recurse -Force
    } else {
      Copy-Item "$TempExtract\*" -Destination $PaddleDir -Recurse -Force
    }

    # Verify
    $paddleSize = [math]::Round((Get-ChildItem $PaddleDir -Recurse | Measure-Object Length -Sum).Sum / 1MB, 1)
    if (-not (Test-Path (Join-Path $PaddleDir "PaddleOCR-json.exe"))) {
      throw "PaddleOCR-json.exe not found after extract"
    }
    Write-Host "  Installed ($paddleSize MB) — verified OK" -ForegroundColor Green
  } catch {
    Write-Warning "  PaddleOCR-json download failed: $_"
    Write-Warning "  OCR will not be available in this build."
    if (Test-Path $PaddleDir) { Remove-Item -Recurse -Force $PaddleDir -ErrorAction SilentlyContinue }
  } finally {
    Remove-Item -Recurse -Force $TempExtract -ErrorAction SilentlyContinue
    Remove-Item $TempArchive -ErrorAction SilentlyContinue
  }
}
Write-Host ""

# ═══════════════════════════════════════════════════════════════════
# 9. Update NSI version + run makensis → Setup.exe
# ═══════════════════════════════════════════════════════════════════

Write-Host "[9/9] Running makensis..." -ForegroundColor Yellow

$NsiFile = "$ReleaseDir\lunac-installer.nsi"
if (-not (Test-Path $NsiFile)) {
  throw "NSI script not found: $NsiFile"
}

# Read + replace version using UTF8 (no BOM) — NSIS compiler requires correct encoding
$NsiContent = [System.IO.File]::ReadAllText($NsiFile, [System.Text.Encoding]::UTF8)

# Replace PRODUCT_VERSION define (OutFile/DisplayVersion use ${PRODUCT_VERSION})
$NewDefine = "!define PRODUCT_VERSION `"$Version`""
$NsiContent = $NsiContent -replace '!define PRODUCT_VERSION "[\d.]+"', $NewDefine

# Verify replacement happened
if ($NsiContent -notmatch [regex]::Escape($NewDefine)) {
  throw "Failed to update PRODUCT_VERSION in NSI script"
}

[System.IO.File]::WriteAllText($NsiFile, $NsiContent, [System.Text.UTF8Encoding]::new($true))
Write-Host "  Version in NSI: $Version" -ForegroundColor DarkGray

$Makensis = "C:\Program Files (x86)\NSIS\Bin\makensis.exe"
if (-not (Test-Path $Makensis)) {
  throw "makensis not found at $Makensis"
}

Push-Location $ReleaseDir
try {
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $output = & $Makensis $NsiFile 2>&1
  Write-Host $output -ForegroundColor DarkGray
  if ($LASTEXITCODE -ne 0) {
    throw "makensis failed (exit $LASTEXITCODE)"
  }
  $sw.Stop()
  Write-Host "  Done in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green
} finally { Pop-Location }

# Remove stale literal-name file from previous broken builds
$StalePath = "$ReleaseDir\Lunac-`${PRODUCT_VERSION}-Setup.exe"
if (Test-Path $StalePath) {
  Remove-Item $StalePath -Force
  Write-Host "  Removed stale: $StalePath" -ForegroundColor DarkGray
}

$SetupPath = "$ReleaseDir\Lunac-$Version-Setup.exe"
if (-not (Test-Path $SetupPath)) {
  # Try glob (exclude broken literal-variable filenames)
  $matches = Get-ChildItem $ReleaseDir "Lunac-*-Setup.exe" |
    Where-Object { $_.Name -notlike '*`$*' -and $_.Name -notlike '*${*' } |
    Sort-Object LastWriteTime -Descending
  if ($matches) {
    $SetupPath = $matches[0].FullName
    Write-Host "  Found installer (alt name): $SetupPath" -ForegroundColor Yellow
  } else {
    throw "Setup.exe not produced in $ReleaseDir — check makensis output above for errors"
  }
}

# ═══════════════════════════════════════════════════════════════════
# Done
# ═══════════════════════════════════════════════════════════════════

$Stopwatch.Stop()
$TotalSec = [math]::Round($Stopwatch.Elapsed.TotalSeconds, 1)
$SetupSize = [math]::Round((Get-Item $SetupPath).Length / 1MB, 1)

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Build complete — $TotalSec s"         -ForegroundColor Cyan
Write-Host "  $SetupPath"                                -ForegroundColor White
Write-Host "  Size : $SetupSize MB"                      -ForegroundColor White
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
